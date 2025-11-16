//! Virtual filesystem tree representation.
//!
//! The vault tree maintains the logical structure of files and directories
//! independent of the underlying storage provider.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use axiomvault_common::{Error, Result, VaultPath};

/// Type of tree node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeType {
    File,
    Directory,
}

/// Metadata for a tree node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMetadata {
    /// Original (cleartext) name.
    pub name: String,
    /// Encrypted name used in storage.
    pub encrypted_name: String,
    /// Node type.
    pub node_type: NodeType,
    /// File size (only for files).
    pub size: Option<u64>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last modification time.
    pub modified_at: DateTime<Utc>,
    /// ETag for conflict detection.
    pub etag: Option<String>,
}

/// A node in the vault tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    /// Unique node identifier.
    pub id: String,
    /// Node metadata.
    pub metadata: NodeMetadata,
    /// Children (for directories).
    pub children: HashMap<String, TreeNode>,
}

impl TreeNode {
    /// Create a new file node.
    pub fn new_file(name: impl Into<String>, encrypted_name: impl Into<String>, size: u64) -> Self {
        let name = name.into();
        let now = Utc::now();

        Self {
            id: Uuid::new_v4().to_string(),
            metadata: NodeMetadata {
                name: name.clone(),
                encrypted_name: encrypted_name.into(),
                node_type: NodeType::File,
                size: Some(size),
                created_at: now,
                modified_at: now,
                etag: Some(Uuid::new_v4().to_string()),
            },
            children: HashMap::new(),
        }
    }

    /// Create a new directory node.
    pub fn new_directory(name: impl Into<String>, encrypted_name: impl Into<String>) -> Self {
        let name = name.into();
        let now = Utc::now();

        Self {
            id: Uuid::new_v4().to_string(),
            metadata: NodeMetadata {
                name: name.clone(),
                encrypted_name: encrypted_name.into(),
                node_type: NodeType::Directory,
                size: None,
                created_at: now,
                modified_at: now,
                etag: Some(Uuid::new_v4().to_string()),
            },
            children: HashMap::new(),
        }
    }

    /// Check if this is a file.
    pub fn is_file(&self) -> bool {
        self.metadata.node_type == NodeType::File
    }

    /// Check if this is a directory.
    pub fn is_directory(&self) -> bool {
        self.metadata.node_type == NodeType::Directory
    }

    /// Get child by name.
    pub fn get_child(&self, name: &str) -> Option<&TreeNode> {
        self.children.get(name)
    }

    /// Get mutable child by name.
    pub fn get_child_mut(&mut self, name: &str) -> Option<&mut TreeNode> {
        self.children.get_mut(name)
    }

    /// Add a child node.
    pub fn add_child(&mut self, node: TreeNode) -> Result<()> {
        if self.is_file() {
            return Err(Error::InvalidInput("Cannot add child to file".to_string()));
        }

        let name = node.metadata.name.clone();
        if self.children.contains_key(&name) {
            return Err(Error::AlreadyExists(format!(
                "Child '{}' already exists",
                name
            )));
        }

        self.children.insert(name, node);
        self.metadata.modified_at = Utc::now();
        Ok(())
    }

    /// Remove a child by name.
    pub fn remove_child(&mut self, name: &str) -> Result<TreeNode> {
        self.children
            .remove(name)
            .ok_or_else(|| Error::NotFound(format!("Child '{}' not found", name)))
    }

    /// List children names.
    pub fn list_children(&self) -> Vec<String> {
        self.children.keys().cloned().collect()
    }
}

/// Virtual filesystem tree for the vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultTree {
    /// Root node.
    root: TreeNode,
}

impl VaultTree {
    /// Create a new empty tree.
    pub fn new() -> Self {
        Self {
            root: TreeNode::new_directory("/", "root"),
        }
    }

    /// Get the root node.
    pub fn root(&self) -> &TreeNode {
        &self.root
    }

    /// Get mutable root node.
    pub fn root_mut(&mut self) -> &mut TreeNode {
        &mut self.root
    }

    /// Navigate to a node by path.
    pub fn get_node(&self, path: &VaultPath) -> Result<&TreeNode> {
        if path.is_root() {
            return Ok(&self.root);
        }

        let mut current = &self.root;
        for component in path.components() {
            current = current
                .get_child(component)
                .ok_or_else(|| Error::NotFound(format!("Path not found: {}", path)))?;
        }

        Ok(current)
    }

    /// Navigate to a mutable node by path.
    pub fn get_node_mut(&mut self, path: &VaultPath) -> Result<&mut TreeNode> {
        if path.is_root() {
            return Ok(&mut self.root);
        }

        let components: Vec<String> = path.components().to_vec();
        let mut current = &mut self.root;

        for component in &components {
            current = current
                .get_child_mut(component)
                .ok_or_else(|| Error::NotFound(format!("Path not found: {}", path)))?;
        }

        Ok(current)
    }

    /// Get parent node for a path.
    pub fn get_parent(&self, path: &VaultPath) -> Result<&TreeNode> {
        match path.parent() {
            Some(parent_path) => self.get_node(&parent_path),
            None => Err(Error::InvalidInput("Root has no parent".to_string())),
        }
    }

    /// Get mutable parent node for a path.
    pub fn get_parent_mut(&mut self, path: &VaultPath) -> Result<&mut TreeNode> {
        match path.parent() {
            Some(parent_path) => self.get_node_mut(&parent_path),
            None => Err(Error::InvalidInput("Root has no parent".to_string())),
        }
    }

    /// Check if a path exists.
    pub fn exists(&self, path: &VaultPath) -> bool {
        self.get_node(path).is_ok()
    }

    /// Create a file in the tree.
    pub fn create_file(
        &mut self,
        path: &VaultPath,
        encrypted_name: impl Into<String>,
        size: u64,
    ) -> Result<()> {
        let name = path
            .name()
            .ok_or_else(|| Error::InvalidInput("Cannot create file at root".to_string()))?;

        let parent = self.get_parent_mut(path)?;
        let node = TreeNode::new_file(name, encrypted_name, size);
        parent.add_child(node)
    }

    /// Create a directory in the tree.
    pub fn create_directory(
        &mut self,
        path: &VaultPath,
        encrypted_name: impl Into<String>,
    ) -> Result<()> {
        let name = path
            .name()
            .ok_or_else(|| Error::InvalidInput("Cannot create directory at root".to_string()))?;

        let parent = self.get_parent_mut(path)?;
        let node = TreeNode::new_directory(name, encrypted_name);
        parent.add_child(node)
    }

    /// Remove a node from the tree.
    pub fn remove(&mut self, path: &VaultPath) -> Result<TreeNode> {
        let name = path
            .name()
            .ok_or_else(|| Error::InvalidInput("Cannot remove root".to_string()))?;

        let parent = self.get_parent_mut(path)?;
        parent.remove_child(name)
    }

    /// List contents of a directory.
    pub fn list(&self, path: &VaultPath) -> Result<Vec<&TreeNode>> {
        let node = self.get_node(path)?;
        if !node.is_directory() {
            return Err(Error::InvalidInput("Not a directory".to_string()));
        }

        Ok(node.children.values().collect())
    }

    /// Serialize tree to JSON.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| Error::Serialization(e.to_string()))
    }

    /// Deserialize tree from JSON.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| Error::Serialization(e.to_string()))
    }

    /// Count the total number of files in the tree.
    pub fn count_files(&self) -> usize {
        self.count_files_recursive(&self.root)
    }

    /// Recursively count files.
    fn count_files_recursive(&self, node: &TreeNode) -> usize {
        let mut count = 0;
        for child in node.children.values() {
            if child.is_file() {
                count += 1;
            } else {
                count += self.count_files_recursive(child);
            }
        }
        count
    }

    /// Get the total size of all files in the tree.
    pub fn total_size(&self) -> u64 {
        self.total_size_recursive(&self.root)
    }

    /// Recursively calculate total size.
    fn total_size_recursive(&self, node: &TreeNode) -> u64 {
        let mut size = 0;
        for child in node.children.values() {
            if child.is_file() {
                size += child.metadata.size.unwrap_or(0);
            } else {
                size += self.total_size_recursive(child);
            }
        }
        size
    }
}

impl Default for VaultTree {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_creation() {
        let tree = VaultTree::new();
        assert!(tree.root().is_directory());
    }

    #[test]
    fn test_create_file() {
        let mut tree = VaultTree::new();
        let path = VaultPath::parse("/test.txt").unwrap();

        tree.create_file(&path, "encrypted", 100).unwrap();

        let node = tree.get_node(&path).unwrap();
        assert!(node.is_file());
        assert_eq!(node.metadata.name, "test.txt");
        assert_eq!(node.metadata.size, Some(100));
    }

    #[test]
    fn test_create_directory() {
        let mut tree = VaultTree::new();
        let path = VaultPath::parse("/mydir").unwrap();

        tree.create_directory(&path, "encrypted_dir").unwrap();

        let node = tree.get_node(&path).unwrap();
        assert!(node.is_directory());
        assert_eq!(node.metadata.name, "mydir");
    }

    #[test]
    fn test_nested_structure() {
        let mut tree = VaultTree::new();

        tree.create_directory(&VaultPath::parse("/dir").unwrap(), "d1")
            .unwrap();
        tree.create_file(&VaultPath::parse("/dir/file.txt").unwrap(), "f1", 50)
            .unwrap();

        let contents = tree.list(&VaultPath::parse("/dir").unwrap()).unwrap();
        assert_eq!(contents.len(), 1);
        assert!(contents[0].is_file());
    }

    #[test]
    fn test_remove_node() {
        let mut tree = VaultTree::new();
        let path = VaultPath::parse("/file.txt").unwrap();

        tree.create_file(&path, "enc", 100).unwrap();
        assert!(tree.exists(&path));

        tree.remove(&path).unwrap();
        assert!(!tree.exists(&path));
    }

    #[test]
    fn test_tree_serialization() {
        let mut tree = VaultTree::new();
        tree.create_directory(&VaultPath::parse("/dir").unwrap(), "d")
            .unwrap();
        tree.create_file(&VaultPath::parse("/dir/f").unwrap(), "e", 10)
            .unwrap();

        let json = tree.to_json().unwrap();
        let restored = VaultTree::from_json(&json).unwrap();

        assert!(restored.exists(&VaultPath::parse("/dir/f").unwrap()));
    }
}
