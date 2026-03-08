"""Predefined test scenarios for AxiomVault computer use testing.

Each scenario has a description and a natural language prompt that
Claude will follow to interact with the application UI.

Scenarios are organized by category:
  - smoke_*        : Basic launch and rendering checks
  - vault_*        : Vault CRUD operations
  - validation_*   : Form validation and error handling
  - files_*        : File/folder operations and navigation
  - modal_*        : Modal dialog interactions
  - ui_*           : UI state, layout, and responsiveness
  - e2e_*          : End-to-end workflows
"""

_SYSTEM_PREFIX = (
    "You are a QA tester for the AxiomVault desktop application. "
    "For each step, report PASS or FAIL with a brief explanation. "
    "Take a screenshot after each action to verify the result. "
    "If a step fails, continue with the remaining steps and report all results at the end. "
)

SCENARIOS = {
    # =========================================================================
    # Smoke tests — basic launch and rendering
    # =========================================================================
    "smoke_test": {
        "description": "Verify the app launches and renders correctly",
        "prompt": _SYSTEM_PREFIX + (
            "Verify the following:\n"
            "1) The header bar shows 'AxiomVault' text\n"
            "2) The left sidebar is visible with a 'Vaults' header\n"
            "3) The sidebar shows 'No Vaults' placeholder text\n"
            "4) The main content area shows 'No Vault Selected' with subtitle "
            "'Select a vault from the sidebar or create a new one.'\n"
            "5) A shield icon is visible in the empty state area\n"
            "6) The sidebar footer has a '+' button and a key button\n"
            "7) The FUSE status text is shown in the top-right area of the toolbar\n"
            "8) There are no error messages, blank screens, or rendering artifacts"
        ),
    },
    "smoke_initial_state": {
        "description": "Verify toolbar buttons are hidden when no vault is selected",
        "prompt": _SYSTEM_PREFIX + (
            "Without selecting any vault, verify:\n"
            "1) The toolbar does NOT show 'New', 'Folder', 'Mount', or 'Lock' buttons\n"
            "2) Only the 'AxiomVault' title and FUSE status text are in the toolbar\n"
            "3) The main area shows the empty state (shield icon + 'No Vault Selected')\n"
            "4) The sidebar footer '+' and key buttons are visible and appear clickable"
        ),
    },

    # =========================================================================
    # Vault CRUD operations
    # =========================================================================
    "vault_create": {
        "description": "Create a single vault and verify it appears",
        "prompt": _SYSTEM_PREFIX + (
            "Perform the following steps:\n"
            "1) Click the '+' button in the sidebar footer to open the Create Vault modal\n"
            "2) Verify the modal title says 'Create New Vault' with subtitle "
            "'Enter details for your new secure container.'\n"
            "3) Verify the form has fields: Vault Name, Password, Verify, Provider\n"
            "4) Verify the Provider dropdown defaults to 'Local Memory'\n"
            "5) Enter 'test-vault' in the Vault Name field\n"
            "6) Enter 'SecurePass123!' in the Password field\n"
            "7) Enter 'SecurePass123!' in the Verify field\n"
            "8) Click the 'Create' button\n"
            "9) Verify the modal closes\n"
            "10) Verify 'test-vault' appears in the sidebar vault list\n"
            "11) Verify the vault is automatically selected (shown in the toolbar title)\n"
            "12) Verify the toolbar now shows 'New', 'Folder', 'Mount', 'Lock' buttons\n"
            "13) Verify a green/colored status dot appears next to the vault in the sidebar"
        ),
    },
    "vault_create_multiple": {
        "description": "Create three vaults and verify all appear in sidebar",
        "prompt": _SYSTEM_PREFIX + (
            "Create three vaults in sequence:\n"
            "1) Create vault 'alpha' with password 'AlphaPass1!', confirm 'AlphaPass1!'\n"
            "2) Verify 'alpha' appears in the sidebar\n"
            "3) Create vault 'bravo' with password 'BravoPass2!', confirm 'BravoPass2!'\n"
            "4) Verify 'bravo' appears in the sidebar\n"
            "5) Create vault 'charlie' with password 'CharlieP3!', confirm 'CharlieP3!'\n"
            "6) Verify 'charlie' appears in the sidebar\n"
            "7) Verify all three vaults are listed in the sidebar simultaneously\n"
            "8) Verify the most recently created vault ('charlie') is the active/selected one"
        ),
    },
    "vault_lock_unlock": {
        "description": "Create a vault, lock it, then unlock it again",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'lock-test' with password 'LockTest99!' (confirm same)\n"
            "2) Verify the vault is selected and toolbar buttons are visible\n"
            "3) Click the red 'Lock' button in the toolbar\n"
            "4) Verify the vault disappears from the sidebar\n"
            "5) Verify the main area returns to 'No Vault Selected' empty state\n"
            "6) Click the key icon button in the sidebar footer to open Unlock modal\n"
            "7) Verify the Unlock modal title says 'Unlock Vault' with subtitle "
            "'Enter your credentials.'\n"
            "8) Enter 'lock-test' in the Vault ID field\n"
            "9) Enter 'LockTest99!' in the Password field\n"
            "10) Click 'Unlock'\n"
            "11) Verify the vault reappears in the sidebar\n"
            "12) Verify the vault is selected and toolbar buttons reappear"
        ),
    },
    "vault_switch": {
        "description": "Create two vaults and switch between them",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'first' with password 'FirstVlt1!' (confirm same)\n"
            "2) Verify 'first' is selected — toolbar title should show 'first'\n"
            "3) Create vault 'second' with password 'SecondVl2!' (confirm same)\n"
            "4) Verify 'second' is now selected — toolbar title should show 'second'\n"
            "5) Click on 'first' in the sidebar\n"
            "6) Verify the toolbar title switches to 'first'\n"
            "7) Verify the file browser content area updates (should show Root)\n"
            "8) Click on 'second' in the sidebar\n"
            "9) Verify the toolbar title switches back to 'second'\n"
            "10) Verify each vault has its own independent file browser state"
        ),
    },

    # =========================================================================
    # Form validation
    # =========================================================================
    "validation_create_empty": {
        "description": "Submit create form with all fields empty",
        "prompt": _SYSTEM_PREFIX + (
            "1) Click '+' to open the Create Vault modal\n"
            "2) Without entering anything, click the 'Create' button\n"
            "3) Verify validation error messages appear:\n"
            "   - 'Vault name is required' under the Vault Name field\n"
            "   - 'Password is required' under the Password field\n"
            "   - 'Password confirmation is required' under the Verify field\n"
            "4) Verify the modal stays open (was NOT submitted)\n"
            "5) Verify the input fields are visually highlighted as having errors "
            "(red border or similar styling)"
        ),
    },
    "validation_create_short_name": {
        "description": "Test vault name minimum length validation (3 chars)",
        "prompt": _SYSTEM_PREFIX + (
            "1) Click '+' to open the Create Vault modal\n"
            "2) Enter 'ab' (only 2 characters) in the Vault Name field\n"
            "3) Enter 'ValidPass1!' in both Password and Verify fields\n"
            "4) Click 'Create'\n"
            "5) Verify the error 'Vault name must be at least 3 characters' appears\n"
            "6) Verify the modal stays open\n"
            "7) Now change the vault name to 'abc' (3 characters)\n"
            "8) Re-enter 'ValidPass1!' in both password fields\n"
            "9) Click 'Create'\n"
            "10) Verify the vault is created successfully and modal closes"
        ),
    },
    "validation_create_short_password": {
        "description": "Test password minimum length validation (8 chars)",
        "prompt": _SYSTEM_PREFIX + (
            "1) Click '+' to open the Create Vault modal\n"
            "2) Enter 'pwd-test' in the Vault Name field\n"
            "3) Enter 'Short1!' (only 7 characters) in the Password field\n"
            "4) Enter 'Short1!' in the Verify field\n"
            "5) Click 'Create'\n"
            "6) Verify the error 'Password must be at least 8 characters' appears\n"
            "7) Verify the modal stays open\n"
            "8) Change password to 'LongEnough1!' in both fields\n"
            "9) Click 'Create'\n"
            "10) Verify the vault is created successfully"
        ),
    },
    "validation_create_mismatch": {
        "description": "Test password mismatch validation",
        "prompt": _SYSTEM_PREFIX + (
            "1) Click '+' to open the Create Vault modal\n"
            "2) Enter 'mismatch-test' in the Vault Name field\n"
            "3) Enter 'Password123!' in the Password field\n"
            "4) Enter 'Different123!' in the Verify field\n"
            "5) Click 'Create'\n"
            "6) Verify the error 'Passwords do not match' appears under the Verify field\n"
            "7) Verify the modal stays open\n"
            "8) Now correct the Verify field to 'Password123!'\n"
            "9) Click 'Create'\n"
            "10) Verify the vault is created successfully"
        ),
    },
    "validation_unlock_empty": {
        "description": "Submit unlock form with empty fields",
        "prompt": _SYSTEM_PREFIX + (
            "1) Click the key icon in the sidebar footer to open Unlock modal\n"
            "2) Without entering anything, click the 'Unlock' button\n"
            "3) Verify 'Vault ID is required' error appears under the Vault ID field\n"
            "4) Verify 'Password is required' error appears under the Password field\n"
            "5) Verify the modal stays open\n"
            "6) Verify input fields show error styling"
        ),
    },
    "validation_error_clears_on_input": {
        "description": "Verify validation errors clear when user starts typing",
        "prompt": _SYSTEM_PREFIX + (
            "1) Click '+' to open the Create Vault modal\n"
            "2) Click 'Create' without entering anything to trigger all validation errors\n"
            "3) Verify error messages are visible under the fields\n"
            "4) Start typing in the Vault Name field (enter 'a')\n"
            "5) Verify ALL validation error messages disappear immediately\n"
            "6) Click 'Create' again\n"
            "7) Verify validation errors reappear (since 'a' is too short, "
            "and password fields are still empty)"
        ),
    },
    "validation_wrong_password_unlock": {
        "description": "Test error display when unlocking with wrong password",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'secure-vault' with password 'CorrectP1!' (confirm same)\n"
            "2) Click 'Lock' to lock the vault\n"
            "3) Click the key icon to open Unlock modal\n"
            "4) Enter 'secure-vault' as Vault ID\n"
            "5) Enter 'WrongPassword!' as Password\n"
            "6) Click 'Unlock'\n"
            "7) Verify an error banner appears at the top of the modal "
            "(with an exclamation icon and error message)\n"
            "8) Verify the modal stays open\n"
            "9) Verify there is an 'X' button to dismiss the error\n"
            "10) Click the 'X' to dismiss the error\n"
            "11) Verify the error banner disappears\n"
            "12) Enter the correct password 'CorrectP1!'\n"
            "13) Click 'Unlock'\n"
            "14) Verify the vault unlocks successfully"
        ),
    },

    # =========================================================================
    # Modal interactions
    # =========================================================================
    "modal_open_close": {
        "description": "Test opening and closing modals via different methods",
        "prompt": _SYSTEM_PREFIX + (
            "1) Click '+' to open the Create Vault modal\n"
            "2) Verify the modal overlay (dark backdrop) is visible\n"
            "3) Click the 'Cancel' button\n"
            "4) Verify the modal closes and the main UI is visible again\n"
            "5) Click '+' to reopen the Create modal\n"
            "6) Click on the dark backdrop area (outside the modal form)\n"
            "7) Verify the modal closes (backdrop click should dismiss)\n"
            "8) Click the key icon to open the Unlock modal\n"
            "9) Verify the Unlock modal opens with 'Unlock Vault' title\n"
            "10) Click 'Cancel' to close it\n"
            "11) Verify modal is closed"
        ),
    },
    "modal_keyboard_submit": {
        "description": "Test submitting forms by pressing Enter",
        "prompt": _SYSTEM_PREFIX + (
            "1) Click '+' to open the Create Vault modal\n"
            "2) Enter 'enter-test' in Vault Name, 'EnterPass1!' in Password, "
            "'EnterPass1!' in Verify\n"
            "3) With focus still in the Verify field, press the Enter key\n"
            "4) Verify the vault is created (modal should close, vault appears in sidebar)\n"
            "5) Lock the vault\n"
            "6) Click the key icon to open Unlock modal\n"
            "7) Enter 'enter-test' and 'EnterPass1!' in the fields\n"
            "8) Press Enter while focused on the password field\n"
            "9) Verify the vault unlocks successfully"
        ),
    },
    "modal_preserves_no_data": {
        "description": "Verify modal form resets when reopened after cancel",
        "prompt": _SYSTEM_PREFIX + (
            "1) Click '+' to open the Create Vault modal\n"
            "2) Enter 'partial-name' in the Vault Name field\n"
            "3) Enter 'partial' in the Password field\n"
            "4) Click 'Cancel' to close the modal without submitting\n"
            "5) Click '+' again to reopen the Create modal\n"
            "6) Check the Vault Name field — does it still contain 'partial-name' "
            "or is it empty/reset?\n"
            "7) Check the Password field — does it still contain 'partial' or is it empty?\n"
            "8) Report whether the form data was preserved or cleared"
        ),
    },
    "modal_provider_dropdown": {
        "description": "Test the provider dropdown in create vault form",
        "prompt": _SYSTEM_PREFIX + (
            "1) Click '+' to open the Create Vault modal\n"
            "2) Verify the Provider dropdown shows 'Local Memory' as default\n"
            "3) Click the Provider dropdown to expand it\n"
            "4) Verify 'Google Drive' is available as an option\n"
            "5) Select 'Google Drive'\n"
            "6) Verify the dropdown now shows 'Google Drive'\n"
            "7) Select 'Local Memory' again\n"
            "8) Verify it switches back to 'Local Memory'\n"
            "9) Click 'Cancel' to close"
        ),
    },

    # =========================================================================
    # File and folder operations
    # =========================================================================
    "files_create_folder": {
        "description": "Create a folder and verify it appears in the file browser",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'folder-test' with password 'FolderTst1!' (confirm same)\n"
            "2) Verify the vault is selected and file browser shows 'Folder is empty'\n"
            "3) Verify the breadcrumb/path bar shows 'Root'\n"
            "4) Click the 'Folder' button in the toolbar\n"
            "5) A browser prompt dialog should appear asking for the folder name\n"
            "6) Type 'my-documents' and press Enter or click OK\n"
            "7) Verify the folder 'my-documents' appears in the file browser\n"
            "8) Verify it has a folder icon (not a file icon)\n"
            "9) Verify 'Folder is empty' is no longer shown"
        ),
    },
    "files_create_file": {
        "description": "Create a file and verify it appears",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'file-test' with password 'FileTest1!' (confirm same)\n"
            "2) Click the 'New' button in the toolbar\n"
            "3) A browser prompt dialog should appear asking for the file name\n"
            "4) Type 'readme.txt' and press Enter or click OK\n"
            "5) Verify 'readme.txt' appears in the file browser\n"
            "6) Verify it has a file icon (not a folder icon)\n"
            "7) Verify the file size shows '0 B' or '--'"
        ),
    },
    "files_navigate_nested": {
        "description": "Create nested directories and navigate in/out",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'nav-test' with password 'NaviTest1!' (confirm same)\n"
            "2) Create a folder called 'level1'\n"
            "3) Click on 'level1' to navigate into it\n"
            "4) Verify the breadcrumb/path bar shows 'Root / level1' or similar\n"
            "5) Verify a '..' entry appears at the top of the file list\n"
            "6) Create another folder called 'level2' inside level1\n"
            "7) Click on 'level2' to navigate deeper\n"
            "8) Verify the breadcrumb updates to show the full path\n"
            "9) Click '..' to go back to level1\n"
            "10) Verify we're back in level1 (level2 folder visible)\n"
            "11) Click '..' again to go back to root\n"
            "12) Verify we're at root (level1 folder visible, no '..' entry)"
        ),
    },
    "files_mixed_content": {
        "description": "Create mix of files and folders, verify sorting",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'sort-test' with password 'SortTest1!' (confirm same)\n"
            "2) Create a file called 'zebra.txt'\n"
            "3) Create a folder called 'alpha-dir'\n"
            "4) Create a file called 'apple.txt'\n"
            "5) Create a folder called 'beta-dir'\n"
            "6) Take a screenshot of the file browser\n"
            "7) Verify the display order is:\n"
            "   - Folders first, sorted alphabetically: 'alpha-dir', 'beta-dir'\n"
            "   - Then files, sorted alphabetically: 'apple.txt', 'zebra.txt'\n"
            "8) Verify folders have folder icons and files have file icons"
        ),
    },
    "files_click_file": {
        "description": "Click a file and verify notification appears",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'click-test' with password 'ClickTst1!' (confirm same)\n"
            "2) Create a file called 'test-doc.txt'\n"
            "3) Click on 'test-doc.txt' in the file browser\n"
            "4) Verify a toast notification appears saying 'Selected: test-doc.txt'\n"
            "5) Verify the notification has a blue check icon (not an error icon)\n"
            "6) Wait a few seconds and verify the notification disappears automatically"
        ),
    },
    "files_isolation_between_vaults": {
        "description": "Verify files in one vault don't appear in another",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'vault-x' with password 'VaultXxx1!' (confirm same)\n"
            "2) Create a folder called 'x-only-folder' in vault-x\n"
            "3) Create a file called 'x-only-file.txt' in vault-x\n"
            "4) Verify both items are visible in vault-x's file browser\n"
            "5) Create vault 'vault-y' with password 'VaultYyy1!' (confirm same)\n"
            "6) Verify vault-y's file browser shows 'Folder is empty'\n"
            "7) Verify 'x-only-folder' and 'x-only-file.txt' are NOT shown in vault-y\n"
            "8) Click on vault-x in the sidebar\n"
            "9) Verify vault-x still shows its files ('x-only-folder', 'x-only-file.txt')"
        ),
    },

    # =========================================================================
    # UI state and layout
    # =========================================================================
    "ui_toolbar_states": {
        "description": "Verify toolbar buttons appear/disappear based on vault selection",
        "prompt": _SYSTEM_PREFIX + (
            "1) With no vault selected, verify the toolbar only shows 'AxiomVault' title "
            "and FUSE status — NO action buttons\n"
            "2) Create vault 'toolbar-test' with password 'Toolbar11!' (confirm same)\n"
            "3) Verify the toolbar now shows four buttons: 'New', 'Folder', 'Mount', 'Lock'\n"
            "4) Verify 'Lock' button appears in red/danger color\n"
            "5) Verify the toolbar title now shows the vault name 'toolbar-test' "
            "with a blue archive icon\n"
            "6) Click 'Lock' to lock the vault\n"
            "7) Verify the toolbar buttons disappear and title returns to 'AxiomVault'"
        ),
    },
    "ui_sidebar_active_highlight": {
        "description": "Verify the active vault is highlighted in the sidebar",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'highlight-a' with password 'Highlight1!' (confirm same)\n"
            "2) Create vault 'highlight-b' with password 'Highlight2!' (confirm same)\n"
            "3) Observe the sidebar — 'highlight-b' should be visually highlighted "
            "(different background color) as the active vault\n"
            "4) Click on 'highlight-a'\n"
            "5) Verify 'highlight-a' becomes highlighted and 'highlight-b' loses highlight\n"
            "6) Click on 'highlight-b'\n"
            "7) Verify the highlight switches back"
        ),
    },
    "ui_vault_status_dots": {
        "description": "Verify vault status indicators in the sidebar",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'status-test' with password 'StatusTst1!' (confirm same)\n"
            "2) Look at the vault entry in the sidebar\n"
            "3) Verify there is a small status dot to the right of the vault name\n"
            "4) The dot should indicate 'Unlocked' status (hover tooltip if visible)\n"
            "5) Report the color and appearance of the status dot"
        ),
    },
    "ui_empty_folder_state": {
        "description": "Verify empty folder message in file browser",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'empty-test' with password 'EmptyTst1!' (confirm same)\n"
            "2) Verify the file browser shows 'Folder is empty' message\n"
            "3) Create a folder called 'subdir'\n"
            "4) Click on 'subdir' to enter it\n"
            "5) Verify 'subdir' also shows 'Folder is empty' (plus the '..' entry)\n"
            "6) Navigate back to root using '..'\n"
            "7) Verify 'Folder is empty' is NOT shown (since 'subdir' exists)"
        ),
    },
    "ui_breadcrumb_display": {
        "description": "Verify breadcrumb path bar updates correctly",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'bread-test' with password 'BreadCrm1!' (confirm same)\n"
            "2) Verify the path bar shows 'Root'\n"
            "3) Create folder 'photos'\n"
            "4) Enter 'photos'\n"
            "5) Verify path bar shows 'Root / photos'\n"
            "6) Create folder 'vacation'\n"
            "7) Enter 'vacation'\n"
            "8) Verify path bar shows 'Root / photos / vacation'\n"
            "9) Navigate back to root using '..' twice\n"
            "10) Verify path bar shows 'Root' again"
        ),
    },
    "ui_toast_success_and_error": {
        "description": "Verify both success and error toast notifications",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'toast-test' with password 'ToastTst1!' (confirm same)\n"
            "2) Verify a success toast appears (e.g., 'Vault created')\n"
            "3) Verify the toast has a blue check icon\n"
            "4) Wait for the toast to disappear (should auto-dismiss in ~3 seconds)\n"
            "5) Click 'Lock' to lock the vault\n"
            "6) Verify a 'Locked' toast appears\n"
            "Report the appearance and behavior of the toast notifications."
        ),
    },
    "ui_loading_spinner": {
        "description": "Verify loading spinner appears during vault creation",
        "prompt": _SYSTEM_PREFIX + (
            "1) Click '+' to open Create Vault modal\n"
            "2) Fill in: name 'spinner-test', password 'SpinnerT1!', confirm 'SpinnerT1!'\n"
            "3) Click 'Create' and IMMEDIATELY take a screenshot\n"
            "4) Check if the Create button text changes to 'Creating...' with a spinner\n"
            "5) Check if the Cancel button becomes disabled/grayed during creation\n"
            "6) After creation completes, verify the modal closes normally\n"
            "Report whether you observed the loading state."
        ),
    },

    # =========================================================================
    # End-to-end workflows
    # =========================================================================
    "e2e_full_lifecycle": {
        "description": "Complete vault lifecycle: create, files, lock, unlock, verify",
        "prompt": _SYSTEM_PREFIX + (
            "Perform a complete vault lifecycle test:\n"
            "1) Create vault 'lifecycle' with password 'LifeCycl1!' (confirm same)\n"
            "2) Verify vault appears in sidebar and is selected\n"
            "3) Create folder 'work'\n"
            "4) Enter the 'work' folder\n"
            "5) Create file 'todo.txt'\n"
            "6) Navigate back to root\n"
            "7) Verify 'work' folder is visible\n"
            "8) Lock the vault\n"
            "9) Verify vault disappears from sidebar\n"
            "10) Verify 'No Vault Selected' empty state is shown\n"
            "11) Unlock the vault with password 'LifeCycl1!'\n"
            "12) Verify vault reappears and is selected\n"
            "13) Verify the file browser shows the 'work' folder (state was preserved)\n"
            "14) Enter 'work' and verify 'todo.txt' is still there\n"
            "Report the result of every step."
        ),
    },
    "e2e_multi_vault_workflow": {
        "description": "Work with multiple vaults simultaneously",
        "prompt": _SYSTEM_PREFIX + (
            "1) Create vault 'personal' with password 'Personal1!' (confirm same)\n"
            "2) Create folder 'photos' in personal vault\n"
            "3) Create file 'selfie.jpg' in personal vault root\n"
            "4) Create vault 'work' with password 'WorkWork1!' (confirm same)\n"
            "5) Create folder 'reports' in work vault\n"
            "6) Create file 'q4-report.pdf' in work vault root\n"
            "7) Verify both vaults are listed in sidebar\n"
            "8) Click 'personal' — verify it shows 'photos' folder and 'selfie.jpg'\n"
            "9) Click 'work' — verify it shows 'reports' folder and 'q4-report.pdf'\n"
            "10) Lock 'personal' vault (select it first, then lock)\n"
            "11) Verify only 'work' remains in sidebar\n"
            "12) Verify 'work' still shows its files\n"
            "13) Unlock 'personal' with its password\n"
            "14) Verify both vaults are in sidebar again"
        ),
    },
    "e2e_rapid_operations": {
        "description": "Perform many operations quickly to test responsiveness",
        "prompt": _SYSTEM_PREFIX + (
            "Perform operations rapidly to stress-test the UI:\n"
            "1) Create vault 'rapid' with password 'RapidTst1!' (confirm same)\n"
            "2) Quickly create 5 folders: 'dir-1', 'dir-2', 'dir-3', 'dir-4', 'dir-5'\n"
            "3) Quickly create 3 files: 'file-a.txt', 'file-b.txt', 'file-c.txt'\n"
            "4) Verify all 8 items appear in the file browser\n"
            "5) Verify folders are sorted before files\n"
            "6) Click into dir-1, create a file 'nested.txt', click '..'\n"
            "7) Click into dir-2, verify it's empty, click '..'\n"
            "8) Verify no UI glitches, missing items, or errors occurred\n"
            "Report any lag, missing items, or rendering issues."
        ),
    },
    "e2e_error_recovery": {
        "description": "Trigger errors and verify the app recovers gracefully",
        "prompt": _SYSTEM_PREFIX + (
            "Test error handling and recovery:\n"
            "1) Try to unlock a vault that doesn't exist: open Unlock modal, "
            "enter 'nonexistent' as ID, 'AnyPass12!' as password, click Unlock\n"
            "2) Verify an error message appears in the modal\n"
            "3) Dismiss the error and close the modal\n"
            "4) Verify the app is still functional (sidebar, empty state visible)\n"
            "5) Create vault 'recover' with password 'Recover11!' (confirm same)\n"
            "6) Verify the vault works normally after the previous error\n"
            "7) Create a folder and file to confirm full functionality\n"
            "8) Lock the vault\n"
            "9) Try to unlock with wrong password 'BadPass123!'\n"
            "10) Verify error appears\n"
            "11) Unlock with correct password 'Recover11!'\n"
            "12) Verify everything still works"
        ),
    },
    "e2e_mount_workflow": {
        "description": "Test the mount/unmount workflow (expects FUSE unavailable)",
        "prompt": _SYSTEM_PREFIX + (
            "1) Check the FUSE status shown in the toolbar top-right area\n"
            "2) Note what the status says (likely 'FUSE support not compiled in' or similar)\n"
            "3) Create vault 'mount-test' with password 'MountTst1!' (confirm same)\n"
            "4) Click the 'Mount' button in the toolbar\n"
            "5) Verify the Mount Volume modal opens with title 'Mount Volume' "
            "and subtitle 'Select a mount point.'\n"
            "6) Enter '/tmp/axiomvault-mount' in the Mount Point field\n"
            "7) Click 'Mount'\n"
            "8) Verify an error appears (since FUSE is likely not available)\n"
            "9) Verify the error message is displayed in the modal error banner\n"
            "10) Close the modal\n"
            "11) Verify the vault is still functional (not corrupted by the failed mount)\n"
            "12) Create a file to confirm vault operations still work"
        ),
    },
}
