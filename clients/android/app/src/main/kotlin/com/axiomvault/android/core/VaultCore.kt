package com.axiomvault.android.core

import com.google.gson.Gson
import com.google.gson.reflect.TypeToken
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.util.concurrent.atomic.AtomicBoolean
import javax.inject.Inject
import javax.inject.Singleton

/**
 * Errors that can occur during vault operations
 */
sealed class VaultError : Exception() {
    data object InitializationFailed : VaultError() {
        private fun readResolve(): Any = InitializationFailed
        override val message = "Failed to initialize AxiomVault"
    }

    data class CreationFailed(override val message: String) : VaultError()
    data class OpenFailed(override val message: String) : VaultError()
    data class OperationFailed(override val message: String) : VaultError()

    data object InvalidHandle : VaultError() {
        private fun readResolve(): Any = InvalidHandle
        override val message = "Invalid vault handle"
    }

    data object InvalidPath : VaultError() {
        private fun readResolve(): Any = InvalidPath
        override val message = "Invalid path"
    }

    data object JsonParsingFailed : VaultError() {
        private fun readResolve(): Any = JsonParsingFailed
        override val message = "Failed to parse JSON response"
    }
}

/**
 * Information about a vault
 */
data class VaultInfo(
    val vaultId: String,
    val rootPath: String,
    val fileCount: Int,
    val totalSize: Long,
    val version: Int
)

/**
 * File or directory entry in the vault
 */
data class VaultEntry(
    val name: String,
    val isDirectory: Boolean,
    val size: Long?
) {
    val id: String = java.util.UUID.randomUUID().toString()
}

/**
 * JSON model for parsing vault entries from native code
 */
private data class VaultEntryJson(
    val name: String,
    val is_directory: Boolean,
    val size: Long?
)

/**
 * Kotlin wrapper for AxiomVault Rust core via JNI
 */
@Singleton
class VaultCore @Inject constructor() {

    companion object {
        private const val LIBRARY_NAME = "axiom_vault"

        init {
            try {
                System.loadLibrary(LIBRARY_NAME)
            } catch (e: UnsatisfiedLinkError) {
                throw RuntimeException("Failed to load native library: $LIBRARY_NAME", e)
            }
        }
    }

    private val initialized = AtomicBoolean(false)
    private var handle: Long = 0L
    private val mutex = Mutex()
    private val gson = Gson()

    // Native methods (JNI bindings to Rust FFI)
    private external fun nativeInit(): Int
    private external fun nativeVersion(): String
    private external fun nativeCreateVault(path: String, password: String): Long
    private external fun nativeOpenVault(path: String, password: String): Long
    private external fun nativeCloseVault(handle: Long): Int
    private external fun nativeVaultInfo(handle: Long): String?
    private external fun nativeVaultList(handle: Long, path: String): String?
    private external fun nativeAddFile(handle: Long, localPath: String, vaultPath: String): Int
    private external fun nativeExtractFile(handle: Long, vaultPath: String, localPath: String): Int
    private external fun nativeMkdir(handle: Long, vaultPath: String): Int
    private external fun nativeRemove(handle: Long, vaultPath: String): Int
    private external fun nativeChangePassword(handle: Long, oldPassword: String, newPassword: String): Int
    private external fun nativeLastError(): String?

    /**
     * Initialize the FFI layer
     */
    suspend fun initialize() = withContext(Dispatchers.IO) {
        mutex.withLock {
            if (initialized.get()) return@withLock

            val result = nativeInit()
            if (result != 0) {
                throw VaultError.InitializationFailed
            }

            initialized.set(true)
        }
    }

    /**
     * Get the library version
     */
    fun version(): String {
        return try {
            nativeVersion()
        } catch (e: Exception) {
            "unknown"
        }
    }

    /**
     * Create a new vault
     */
    suspend fun createVault(path: String, password: String) = withContext(Dispatchers.IO) {
        mutex.withLock {
            if (!initialized.get()) {
                throw VaultError.InitializationFailed
            }

            // Close existing vault if open
            if (handle != 0L) {
                nativeCloseVault(handle)
                handle = 0L
            }

            val newHandle = nativeCreateVault(path, password)
            if (newHandle == 0L) {
                val error = nativeLastError() ?: "Unknown error"
                throw VaultError.CreationFailed(error)
            }

            handle = newHandle
        }
    }

    /**
     * Open an existing vault
     */
    suspend fun openVault(path: String, password: String) = withContext(Dispatchers.IO) {
        mutex.withLock {
            if (!initialized.get()) {
                throw VaultError.InitializationFailed
            }

            // Close existing vault if open
            if (handle != 0L) {
                nativeCloseVault(handle)
                handle = 0L
            }

            val newHandle = nativeOpenVault(path, password)
            if (newHandle == 0L) {
                val error = nativeLastError() ?: "Unknown error"
                throw VaultError.OpenFailed(error)
            }

            handle = newHandle
        }
    }

    /**
     * Close the current vault
     */
    suspend fun closeVault() = withContext(Dispatchers.IO) {
        mutex.withLock {
            if (handle != 0L) {
                nativeCloseVault(handle)
                handle = 0L
            }
        }
    }

    /**
     * Get vault information
     */
    suspend fun getVaultInfo(): VaultInfo = withContext(Dispatchers.IO) {
        mutex.withLock {
            if (handle == 0L) {
                throw VaultError.InvalidHandle
            }

            val jsonStr = nativeVaultInfo(handle)
                ?: throw VaultError.OperationFailed(nativeLastError() ?: "Unknown error")

            try {
                val json = gson.fromJson(jsonStr, Map::class.java)
                VaultInfo(
                    vaultId = json["vault_id"] as? String ?: "",
                    rootPath = json["root_path"] as? String ?: "",
                    fileCount = (json["file_count"] as? Double)?.toInt() ?: 0,
                    totalSize = (json["total_size"] as? Double)?.toLong() ?: 0L,
                    version = (json["version"] as? Double)?.toInt() ?: 1
                )
            } catch (e: Exception) {
                throw VaultError.JsonParsingFailed
            }
        }
    }

    /**
     * List contents of a directory in the vault
     */
    suspend fun listDirectory(path: String = "/"): List<VaultEntry> = withContext(Dispatchers.IO) {
        mutex.withLock {
            if (handle == 0L) {
                throw VaultError.InvalidHandle
            }

            val jsonStr = nativeVaultList(handle, path)
                ?: throw VaultError.OperationFailed(nativeLastError() ?: "Unknown error")

            try {
                val type = object : TypeToken<List<VaultEntryJson>>() {}.type
                val entries: List<VaultEntryJson> = gson.fromJson(jsonStr, type)
                entries.map { entry ->
                    VaultEntry(
                        name = entry.name,
                        isDirectory = entry.is_directory,
                        size = entry.size
                    )
                }
            } catch (e: Exception) {
                throw VaultError.JsonParsingFailed
            }
        }
    }

    /**
     * Add a file to the vault
     */
    suspend fun addFile(localPath: String, vaultPath: String) = withContext(Dispatchers.IO) {
        mutex.withLock {
            if (handle == 0L) {
                throw VaultError.InvalidHandle
            }

            val result = nativeAddFile(handle, localPath, vaultPath)
            if (result != 0) {
                val error = nativeLastError() ?: "Unknown error"
                throw VaultError.OperationFailed(error)
            }
        }
    }

    /**
     * Extract a file from the vault
     */
    suspend fun extractFile(vaultPath: String, localPath: String) = withContext(Dispatchers.IO) {
        mutex.withLock {
            if (handle == 0L) {
                throw VaultError.InvalidHandle
            }

            val result = nativeExtractFile(handle, vaultPath, localPath)
            if (result != 0) {
                val error = nativeLastError() ?: "Unknown error"
                throw VaultError.OperationFailed(error)
            }
        }
    }

    /**
     * Create a directory in the vault
     */
    suspend fun createDirectory(vaultPath: String) = withContext(Dispatchers.IO) {
        mutex.withLock {
            if (handle == 0L) {
                throw VaultError.InvalidHandle
            }

            val result = nativeMkdir(handle, vaultPath)
            if (result != 0) {
                val error = nativeLastError() ?: "Unknown error"
                throw VaultError.OperationFailed(error)
            }
        }
    }

    /**
     * Remove a file or directory from the vault
     */
    suspend fun removeEntry(vaultPath: String) = withContext(Dispatchers.IO) {
        mutex.withLock {
            if (handle == 0L) {
                throw VaultError.InvalidHandle
            }

            val result = nativeRemove(handle, vaultPath)
            if (result != 0) {
                val error = nativeLastError() ?: "Unknown error"
                throw VaultError.OperationFailed(error)
            }
        }
    }

    /**
     * Change the vault password
     */
    suspend fun changePassword(oldPassword: String, newPassword: String) = withContext(Dispatchers.IO) {
        mutex.withLock {
            if (handle == 0L) {
                throw VaultError.InvalidHandle
            }

            val result = nativeChangePassword(handle, oldPassword, newPassword)
            if (result != 0) {
                val error = nativeLastError() ?: "Unknown error"
                throw VaultError.OperationFailed(error)
            }
        }
    }

    /**
     * Check if a vault is currently open
     */
    val isVaultOpen: Boolean
        get() = handle != 0L
}
