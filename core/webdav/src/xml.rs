//! WebDAV XML response generation.
//!
//! Generates RFC 4918 compliant XML for PROPFIND multi-status responses.
//! Uses string formatting instead of a heavy XML crate since WebDAV XML
//! is structurally simple.

use chrono::{DateTime, Utc};

/// A single resource entry for a PROPFIND response.
pub struct PropEntry {
    /// URL-encoded href path (e.g. `/path/to/file.txt`).
    pub href: String,
    /// Display name of the resource.
    pub display_name: String,
    /// Whether this is a collection (directory).
    pub is_collection: bool,
    /// Content length in bytes (files only).
    pub content_length: Option<u64>,
    /// MIME content type.
    pub content_type: String,
    /// Last modification time.
    pub last_modified: DateTime<Utc>,
    /// Creation time.
    pub created: DateTime<Utc>,
    /// ETag value (if available).
    pub etag: Option<String>,
}

/// Build a complete `207 Multi-Status` XML body from a list of property entries.
pub fn build_multistatus(entries: &[PropEntry]) -> String {
    let mut xml = String::with_capacity(1024);
    xml.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    xml.push_str("<D:multistatus xmlns:D=\"DAV:\">\n");

    for entry in entries {
        xml.push_str("  <D:response>\n");
        xml.push_str("    <D:href>");
        xml.push_str(&xml_escape(&entry.href));
        xml.push_str("</D:href>\n");
        xml.push_str("    <D:propstat>\n");
        xml.push_str("      <D:prop>\n");

        // displayname
        xml.push_str("        <D:displayname>");
        xml.push_str(&xml_escape(&entry.display_name));
        xml.push_str("</D:displayname>\n");

        // resourcetype
        if entry.is_collection {
            xml.push_str("        <D:resourcetype><D:collection/></D:resourcetype>\n");
        } else {
            xml.push_str("        <D:resourcetype/>\n");
        }

        // getcontentlength (files only)
        if let Some(len) = entry.content_length {
            xml.push_str("        <D:getcontentlength>");
            xml.push_str(&len.to_string());
            xml.push_str("</D:getcontentlength>\n");
        }

        // getcontenttype
        xml.push_str("        <D:getcontenttype>");
        xml.push_str(&xml_escape(&entry.content_type));
        xml.push_str("</D:getcontenttype>\n");

        // getlastmodified (RFC 2822)
        xml.push_str("        <D:getlastmodified>");
        xml.push_str(
            &entry
                .last_modified
                .format("%a, %d %b %Y %H:%M:%S GMT")
                .to_string(),
        );
        xml.push_str("</D:getlastmodified>\n");

        // creationdate (ISO 8601)
        xml.push_str("        <D:creationdate>");
        xml.push_str(&entry.created.to_rfc3339());
        xml.push_str("</D:creationdate>\n");

        // getetag
        if let Some(ref etag) = entry.etag {
            xml.push_str("        <D:getetag>\"");
            xml.push_str(&xml_escape(etag));
            xml.push_str("\"</D:getetag>\n");
        }

        xml.push_str("      </D:prop>\n");
        xml.push_str("      <D:status>HTTP/1.1 200 OK</D:status>\n");
        xml.push_str("    </D:propstat>\n");
        xml.push_str("  </D:response>\n");
    }

    xml.push_str("</D:multistatus>\n");
    xml
}

/// Escape special XML characters.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("<test>&\"'"), "&lt;test&gt;&amp;&quot;&apos;");
    }

    #[test]
    fn test_build_multistatus_file() {
        let now = Utc::now();
        let entries = vec![PropEntry {
            href: "/test.txt".to_string(),
            display_name: "test.txt".to_string(),
            is_collection: false,
            content_length: Some(42),
            content_type: "text/plain".to_string(),
            last_modified: now,
            created: now,
            etag: Some("abc123".to_string()),
        }];

        let xml = build_multistatus(&entries);
        assert!(xml.contains("<D:multistatus"));
        assert!(xml.contains("<D:href>/test.txt</D:href>"));
        assert!(xml.contains("<D:getcontentlength>42</D:getcontentlength>"));
        assert!(xml.contains("<D:resourcetype/>"));
        assert!(xml.contains("<D:getetag>\"abc123\"</D:getetag>"));
    }

    #[test]
    fn test_build_multistatus_collection() {
        let now = Utc::now();
        let entries = vec![PropEntry {
            href: "/docs/".to_string(),
            display_name: "docs".to_string(),
            is_collection: true,
            content_length: None,
            content_type: "httpd/unix-directory".to_string(),
            last_modified: now,
            created: now,
            etag: None,
        }];

        let xml = build_multistatus(&entries);
        assert!(xml.contains("<D:collection/>"));
        assert!(!xml.contains("<D:getcontentlength>"));
        assert!(!xml.contains("<D:getetag>"));
    }
}
