// Wait for Tauri API to be ready
async function waitForTauri(timeout = 5000) {
    const startTime = Date.now();
    while (!window.__TAURI__ || !window.__TAURI__.core) {
        if (Date.now() - startTime > timeout) {
            throw new Error('Tauri API not available after timeout');
        }
        await new Promise(resolve => setTimeout(resolve, 50));
    }
    return window.__TAURI__.core;
}

// Show error message to user
function showInitError(error) {
    console.error('Failed to initialize application:', error);
    const errorHTML = `
        <div style="color: white; padding: 40px; text-align: center; font-family: -apple-system, BlinkMacSystemFont, sans-serif;">
            <h2 style="color: #ff453a; margin-bottom: 20px;">Initialization Error</h2>
            <p style="font-size: 14px; margin-bottom: 10px;">${error.message}</p>
            <p style="font-size: 12px; color: #98989d;">Check the browser console (F12) for more details.</p>
            <button onclick="location.reload()" style="margin-top: 20px; padding: 8px 16px; background: #0a84ff; color: white; border: none; border-radius: 6px; cursor: pointer;">
                Reload
            </button>
        </div>
    `;
    document.body.innerHTML = errorHTML;
}

// Initialize and mount the Vue application
async function initApp() {
    try {
        console.log('Waiting for Tauri API...');
        const tauriCore = await waitForTauri();
        const { invoke } = tauriCore;
        console.log('Tauri API ready');

        console.log('Loading Vue...');
        const { createApp, ref, computed, onMounted } = await import('https://unpkg.com/vue@3/dist/vue.esm-browser.js');
        console.log('Vue loaded');

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

                const showModal = (type) => activeModal.value = type;
                const closeModal = () => activeModal.value = null;

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
                    const f = forms.value.create;
                    if (!f.name || !f.password) return showNotify('Fill all fields', true);
                    if (f.password !== f.confirm) return showNotify('Passwords match error', true);

                    try {
                        await invoke('create_vault', { id: f.name, password: f.password, providerType: f.provider });
                        closeModal();
                        showNotify('Vault created');
                        await refreshVaults();
                        selectVault(f.name);
                        // Reset form
                        forms.value.create = { name: '', password: '', confirm: '', provider: 'memory' };
                    } catch (e) {
                        showNotify(e, true);
                    }
                };

                const unlockVault = async () => {
                    const f = forms.value.unlock;
                    try {
                        await invoke('unlock_vault', { id: f.id, password: f.password });
                        closeModal();
                        showNotify('Unlocked');
                        await refreshVaults();
                        selectVault(f.id);
                        forms.value.unlock = { id: '', password: '' };
                    } catch (e) {
                        showNotify(e, true);
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
                    try {
                        await invoke('mount_vault', { id: currentVaultId.value, mountPoint: forms.value.mount.point });
                        closeModal();
                        showNotify('Mounted');
                        await refreshVaults();
                        forms.value.mount.point = '';
                    } catch (e) {
                        showNotify(e, true);
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
                    activeModal, forms, notification,
                    showModal, closeModal, createVault, unlockVault, lockVault,
                    toggleMount, mountVault, handleItemClick, navigateUp,
                    createNewFile, createNewFolder, formatSize, selectVault
                };
            }
        });

        console.log('Mounting Vue app...');
        app.mount('#app');
        console.log('Vue app mounted successfully');
    } catch (error) {
        showInitError(error);
    }
}

// Start the application
initApp();
