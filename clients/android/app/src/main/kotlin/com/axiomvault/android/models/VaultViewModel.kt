package com.axiomvault.android.models

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.axiomvault.android.core.VaultCore
import com.axiomvault.android.core.VaultEntry
import com.axiomvault.android.core.VaultError
import com.axiomvault.android.core.VaultInfo
import dagger.hilt.android.lifecycle.HiltViewModel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import javax.inject.Inject

data class VaultState(
    val isInitialized: Boolean = false,
    val isVaultOpen: Boolean = false,
    val vaultInfo: VaultInfo? = null,
    val currentPath: String = "/",
    val entries: List<VaultEntry> = emptyList(),
    val isLoading: Boolean = false,
    val error: String? = null
)

@HiltViewModel
class VaultViewModel @Inject constructor(
    private val vaultCore: VaultCore
) : ViewModel() {

    private val _state = MutableStateFlow(VaultState())
    val state: StateFlow<VaultState> = _state.asStateFlow()

    init {
        initializeCore()
    }

    private fun initializeCore() {
        viewModelScope.launch {
            _state.value = _state.value.copy(isLoading = true)
            try {
                vaultCore.initialize()
                _state.value = _state.value.copy(
                    isInitialized = true,
                    isLoading = false
                )
            } catch (e: Exception) {
                _state.value = _state.value.copy(
                    isLoading = false,
                    error = "Failed to initialize: ${e.message}"
                )
            }
        }
    }

    fun createVault(path: String, password: String) {
        viewModelScope.launch {
            _state.value = _state.value.copy(isLoading = true, error = null)
            try {
                vaultCore.createVault(path, password)
                val info = vaultCore.getVaultInfo()
                _state.value = _state.value.copy(
                    isVaultOpen = true,
                    vaultInfo = info,
                    currentPath = "/",
                    isLoading = false
                )
                loadEntries("/")
            } catch (e: VaultError) {
                _state.value = _state.value.copy(
                    isLoading = false,
                    error = e.message
                )
            }
        }
    }

    fun openVault(path: String, password: String) {
        viewModelScope.launch {
            _state.value = _state.value.copy(isLoading = true, error = null)
            try {
                vaultCore.openVault(path, password)
                val info = vaultCore.getVaultInfo()
                _state.value = _state.value.copy(
                    isVaultOpen = true,
                    vaultInfo = info,
                    currentPath = "/",
                    isLoading = false
                )
                loadEntries("/")
            } catch (e: VaultError) {
                _state.value = _state.value.copy(
                    isLoading = false,
                    error = e.message
                )
            }
        }
    }

    fun closeVault() {
        viewModelScope.launch {
            try {
                vaultCore.closeVault()
                _state.value = _state.value.copy(
                    isVaultOpen = false,
                    vaultInfo = null,
                    currentPath = "/",
                    entries = emptyList()
                )
            } catch (e: Exception) {
                _state.value = _state.value.copy(
                    error = "Failed to close vault: ${e.message}"
                )
            }
        }
    }

    fun navigateToPath(path: String) {
        viewModelScope.launch {
            _state.value = _state.value.copy(currentPath = path)
            loadEntries(path)
        }
    }

    fun navigateUp() {
        val currentPath = _state.value.currentPath
        if (currentPath != "/") {
            val parentPath = currentPath.substringBeforeLast("/").ifEmpty { "/" }
            navigateToPath(parentPath)
        }
    }

    private fun loadEntries(path: String) {
        viewModelScope.launch {
            _state.value = _state.value.copy(isLoading = true, error = null)
            try {
                val entries = vaultCore.listDirectory(path)
                _state.value = _state.value.copy(
                    entries = entries,
                    isLoading = false
                )
            } catch (e: Exception) {
                _state.value = _state.value.copy(
                    isLoading = false,
                    error = "Failed to list directory: ${e.message}"
                )
            }
        }
    }

    fun addFile(localPath: String, vaultPath: String) {
        viewModelScope.launch {
            _state.value = _state.value.copy(isLoading = true, error = null)
            try {
                vaultCore.addFile(localPath, vaultPath)
                loadEntries(_state.value.currentPath)
            } catch (e: Exception) {
                _state.value = _state.value.copy(
                    isLoading = false,
                    error = "Failed to add file: ${e.message}"
                )
            }
        }
    }

    fun extractFile(vaultPath: String, localPath: String) {
        viewModelScope.launch {
            _state.value = _state.value.copy(isLoading = true, error = null)
            try {
                vaultCore.extractFile(vaultPath, localPath)
                _state.value = _state.value.copy(isLoading = false)
            } catch (e: Exception) {
                _state.value = _state.value.copy(
                    isLoading = false,
                    error = "Failed to extract file: ${e.message}"
                )
            }
        }
    }

    fun createDirectory(name: String) {
        viewModelScope.launch {
            _state.value = _state.value.copy(isLoading = true, error = null)
            try {
                val fullPath = if (_state.value.currentPath == "/") {
                    "/$name"
                } else {
                    "${_state.value.currentPath}/$name"
                }
                vaultCore.createDirectory(fullPath)
                loadEntries(_state.value.currentPath)
            } catch (e: Exception) {
                _state.value = _state.value.copy(
                    isLoading = false,
                    error = "Failed to create directory: ${e.message}"
                )
            }
        }
    }

    fun removeEntry(vaultPath: String) {
        viewModelScope.launch {
            _state.value = _state.value.copy(isLoading = true, error = null)
            try {
                vaultCore.removeEntry(vaultPath)
                loadEntries(_state.value.currentPath)
            } catch (e: Exception) {
                _state.value = _state.value.copy(
                    isLoading = false,
                    error = "Failed to remove entry: ${e.message}"
                )
            }
        }
    }

    fun clearError() {
        _state.value = _state.value.copy(error = null)
    }

    fun getVersion(): String = vaultCore.version()
}
