// Tauri API resolution — does not block Vue from mounting
let _invoke = null;

function getTauriInvoke() {
    // Use the internal IPC bridge (withGlobalTauri is disabled for security)
    if (window.__TAURI_INTERNALS__?.invoke) return window.__TAURI_INTERNALS__.invoke;
    return null;
}

let _tauriReady = null;
function waitForTauri() {
    if (_tauriReady) return _tauriReady;
    _tauriReady = new Promise((resolve) => {
        // Check immediately — the IPC bridge may already be available
        const fn = getTauriInvoke();
        if (fn) {
            console.log('[Tauri] API available immediately');
            _invoke = fn;
            resolve(fn);
            return;
        }

        const timeout = 5000;
        const startTime = Date.now();
        const check = setInterval(() => {
            const fn = getTauriInvoke();
            if (fn) {
                clearInterval(check);
                console.log(`[Tauri] API ready after ${Date.now() - startTime}ms`);
                _invoke = fn;
                resolve(fn);
            } else if (Date.now() - startTime > timeout) {
                clearInterval(check);
                console.error('[Tauri] API not available after timeout');
                resolve(null);
            }
        }, 25);
    });
    return _tauriReady;
}

// Safe invoke wrapper — waits for Tauri, throws descriptive error if unavailable
async function safeInvoke(cmd, args) {
    if (!_invoke) {
        const fn = await waitForTauri();
        if (!fn) throw 'Tauri backend is not available. Try reloading the app.';
    }
    return _invoke(cmd, args);
}

// Initialize and mount the Vue application
function initApp() {
    const startTime = Date.now();
    console.log('[App Init] Starting initialization at', new Date().toISOString());

    // Start Tauri resolution in background (non-blocking)
    waitForTauri();

    const { createApp, ref, computed, onMounted } = Vue;
    const invoke = safeInvoke;
    console.log(`[App Init] ✓ Vue loaded (${Date.now() - startTime}ms)`);

    const app = createApp({
        setup() {
                // State
                const fuseStatus = ref('Checking...');
                const vaults = ref([]);
                const currentVaultId = ref(null);
                const currentPath = ref('/');
                const files = ref([]);
                const activeModal = ref(null); // 'create', 'unlock', 'mount', or null

                const forms = ref({
                    create: { name: '', password: '', confirm: '', provider: 'memory' },
                    unlock: { id: '', password: '' },
                    mount: { point: '' }
                });

                const notification = ref({ show: false, message: '', isError: false });

                // Loading states and validation
                const isLoading = ref(false);
                const validationErrors = ref({
                    create: {},
                    unlock: {},
                    mount: {}
                });
                const modalErrors = ref({
                    create: null,
                    unlock: null,
                    mount: null
                });

                // Computed
                const breadcrumbs = computed(() => currentPath.value.split('/'));

                const sortedFiles = computed(() => {
                    return [...files.value].sort((a, b) => {
                        if (a.is_directory === b.is_directory) return a.name.localeCompare(b.name);
                        return a.is_directory ? -1 : 1;
                    });
                });

                // Actions
                const showNotify = (msg, isError = false) => {
                    notification.value = { show: true, message: msg, isError };
                    setTimeout(() => notification.value.show = false, 3000);
                };

                // Form validation functions
                const validateCreateForm = () => {
                    const errors = {};
                    const f = forms.value.create;

                    if (!f.name || f.name.trim().length === 0) {
                        errors.name = 'Vault name is required';
                    } else if (f.name.trim().length < 3) {
                        errors.name = 'Vault name must be at least 3 characters';
                    }

                    if (!f.password || f.password.length === 0) {
                        errors.password = 'Password is required';
                    } else if (f.password.length < 8) {
                        errors.password = 'Password must be at least 8 characters';
                    }

                    if (!f.confirm || f.confirm.length === 0) {
                        errors.confirm = 'Password confirmation is required';
                    } else if (f.password !== f.confirm) {
                        errors.confirm = 'Passwords do not match';
                    }

                    validationErrors.value.create = errors;
                    return Object.keys(errors).length === 0;
                };

                const validateUnlockForm = () => {
                    const errors = {};
                    const f = forms.value.unlock;

                    if (!f.id || f.id.trim().length === 0) {
                        errors.id = 'Vault ID is required';
                    }

                    if (!f.password || f.password.length === 0) {
                        errors.password = 'Password is required';
                    }

                    validationErrors.value.unlock = errors;
                    return Object.keys(errors).length === 0;
                };

                const validateMountForm = () => {
                    const errors = {};
                    const f = forms.value.mount;

                    if (!f.point || f.point.trim().length === 0) {
                        errors.point = 'Mount point is required';
                    }

                    validationErrors.value.mount = errors;
                    return Object.keys(errors).length === 0;
                };

                // Clear validation errors and modal errors when form changes
                const clearValidationErrors = (formType) => {
                    validationErrors.value[formType] = {};
                    modalErrors.value[formType] = null;
                };

                const showModal = (type) => {
                    activeModal.value = type;
                    // Clear any existing errors when opening modal
                    clearValidationErrors(type);
                };

                const closeModal = () => {
                    activeModal.value = null;
                    // Clear all validation errors and modal errors when closing
                    Object.keys(validationErrors.value).forEach(key => {
                        validationErrors.value[key] = {};
                    });
                    Object.keys(modalErrors.value).forEach(key => {
                        modalErrors.value[key] = null;
                    });
                };

                // API Calls
                const refreshVaults = async () => {
                    try {
                        vaults.value = await invoke('list_vaults');
                    } catch (e) {
                        showNotify('Failed to load vaults: ' + e, true);
                    }
                };

                const refreshFiles = async () => {
                    if (!currentVaultId.value) return;
                    try {
                        files.value = await invoke('list_files', {
                            id: currentVaultId.value,
                            path: currentPath.value
                        });
                    } catch (e) {
                        showNotify('Failed to list files: ' + e, true);
                    }
                };

                const selectVault = async (id) => {
                    currentVaultId.value = id;
                    currentPath.value = '/';
                    await refreshFiles();
                };

                // CRUD Operations
                const createVault = async () => {
                    // Clear any existing modal errors
                    modalErrors.value.create = null;

                    // Validate form
                    if (!validateCreateForm()) {
                        return;
                    }

                    const f = forms.value.create;
                    isLoading.value = true;

                    try {
                        await invoke('create_vault', { id: f.name, password: f.password, providerType: f.provider });
                        closeModal();
                        showNotify('Vault created');
                        await refreshVaults();
                        selectVault(f.name);
                        // Reset form
                        forms.value.create = { name: '', password: '', confirm: '', provider: 'memory' };
                    } catch (e) {
                        modalErrors.value.create = typeof e === 'string' ? e : e.toString();
                    } finally {
                        isLoading.value = false;
                    }
                };

                const unlockVault = async () => {
                    // Clear any existing modal errors
                    modalErrors.value.unlock = null;

                    // Validate form
                    if (!validateUnlockForm()) {
                        return;
                    }

                    const f = forms.value.unlock;
                    isLoading.value = true;

                    try {
                        await invoke('unlock_vault', { id: f.id, password: f.password });
                        closeModal();
                        showNotify('Unlocked');
                        await refreshVaults();
                        selectVault(f.id);
                        forms.value.unlock = { id: '', password: '' };
                    } catch (e) {
                        modalErrors.value.unlock = typeof e === 'string' ? e : e.toString();
                    } finally {
                        isLoading.value = false;
                    }
                };

                const lockVault = async () => {
                    try {
                        await invoke('lock_vault', { id: currentVaultId.value });
                        currentVaultId.value = null;
                        showNotify('Locked');
                        await refreshVaults();
                    } catch (e) {
                        showNotify(e, true);
                    }
                };

                const toggleMount = () => {
                    const v = vaults.value.find(v => v.id === currentVaultId.value);
                    if (v && v.is_mounted) unmountVault();
                    else showModal('mount');
                };

                const mountVault = async () => {
                    // Clear any existing modal errors
                    modalErrors.value.mount = null;

                    // Validate form
                    if (!validateMountForm()) {
                        return;
                    }

                    isLoading.value = true;

                    try {
                        await invoke('mount_vault', { id: currentVaultId.value, mountPoint: forms.value.mount.point });
                        closeModal();
                        showNotify('Mounted');
                        await refreshVaults();
                        forms.value.mount.point = '';
                    } catch (e) {
                        modalErrors.value.mount = typeof e === 'string' ? e : e.toString();
                    } finally {
                        isLoading.value = false;
                    }
                };

                const unmountVault = async () => {
                    try {
                        await invoke('unmount_vault', { id: currentVaultId.value });
                        showNotify('Unmounted');
                        await refreshVaults();
                    } catch (e) {
                        showNotify(e, true);
                    }
                };

                // File System Ops
                const handleItemClick = (file) => {
                    if (file.is_directory) {
                        currentPath.value = file.path;
                        refreshFiles();
                    } else {
                        showNotify('Selected: ' + file.name);
                    }
                };

                const navigateUp = () => {
                    const parts = currentPath.value.split('/').filter(p => p);
                    parts.pop();
                    currentPath.value = parts.length === 0 ? '/' : '/' + parts.join('/');
                    refreshFiles();
                };

                const createNewFile = async () => {
                    const name = prompt('File name:');
                    if (!name) return;
                    const path = currentPath.value === '/' ? '/' + name : currentPath.value + '/' + name;
                    try {
                        await invoke('create_file', { vaultId: currentVaultId.value, path, content: [] });
                        refreshFiles();
                    } catch(e) { showNotify(e, true); }
                };

                const createNewFolder = async () => {
                    const name = prompt('Folder name:');
                    if (!name) return;
                    const path = currentPath.value === '/' ? '/' + name : currentPath.value + '/' + name;
                    try {
                        await invoke('create_directory', { vaultId: currentVaultId.value, path });
                        refreshFiles();
                    } catch(e) { showNotify(e, true); }
                };

                // Utils
                const formatSize = (bytes) => {
                    if (bytes < 1024) return bytes + ' B';
                    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
                    return (bytes / 1024 / 1024).toFixed(1) + ' MB';
                };

                // Init
                onMounted(async () => {
                    try {
                        fuseStatus.value = await invoke('get_fuse_info');
                    } catch { fuseStatus.value = 'Offline'; }
                    await refreshVaults();
                });

                return {
                    fuseStatus, vaults, currentVaultId, currentPath, files, breadcrumbs, sortedFiles,
                    activeModal, forms, notification, isLoading, validationErrors, modalErrors,
                    showModal, closeModal, createVault, unlockVault, lockVault,
                    toggleMount, mountVault, handleItemClick, navigateUp,
                    createNewFile, createNewFolder, formatSize, selectVault,
                    clearValidationErrors
                };
        }
    });

    console.log(`[App Init] Mounting Vue app (${Date.now() - startTime}ms)...`);
    app.mount('#app');
    console.log(`[App Init] ✓ Vue app mounted successfully (${Date.now() - startTime}ms total)`);
}

// Start the application
console.log('[Bootstrap] Calling initApp()...');
initApp();
