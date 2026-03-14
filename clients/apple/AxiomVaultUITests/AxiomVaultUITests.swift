import XCTest

final class AxiomVaultUITests: XCTestCase {
    var app: XCUIApplication!

    override func setUpWithError() throws {
        continueAfterFailure = false
        app = XCUIApplication()
        app.launchArguments = ["--uitesting", "--reset-state"]
        app.launch()
    }

    override func tearDownWithError() throws {
        app = nil
    }

    // MARK: - Landing Screen

    func testLandingScreenShowsAppTitle() {
        XCTAssertTrue(app.staticTexts["AxiomVault"].waitForExistence(timeout: 5))
    }

    func testLandingScreenShowsSubtitle() {
        XCTAssertTrue(app.staticTexts["Secure, encrypted file storage"].exists)
    }

    func testLandingScreenShowsCreateButton() {
        XCTAssertTrue(app.buttons["Create New Vault"].exists)
    }

    func testLandingScreenShowsOpenButton() {
        XCTAssertTrue(app.buttons["Open Existing Vault"].exists)
    }

    func testLandingScreenShowsShieldIcon() {
        XCTAssertTrue(app.images["lock.shield.fill"].exists)
    }

    // MARK: - Create Vault Flow

    func testCreateVaultSheetAppears() {
        app.buttons["Create New Vault"].tap()

        XCTAssertTrue(app.navigationBars["Create New Vault"].waitForExistence(timeout: 3))
    }

    func testCreateVaultHasRequiredFields() {
        app.buttons["Create New Vault"].tap()

        XCTAssertTrue(app.textFields["My Vault"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.secureTextFields.matching(identifier: "passwordField").firstMatch.exists)
        XCTAssertTrue(app.secureTextFields.matching(identifier: "confirmPasswordField").firstMatch.exists)
    }

    func testCreateVaultButtonDisabledWhenEmpty() {
        app.buttons["Create New Vault"].tap()

        let createButton = app.buttons["Create Vault"]
        XCTAssertTrue(createButton.waitForExistence(timeout: 3))
        XCTAssertFalse(createButton.isEnabled)
    }

    func testCreateVaultShowPasswordToggle() {
        app.buttons["Create New Vault"].tap()

        let secureField = app.secureTextFields.matching(identifier: "passwordField").firstMatch
        XCTAssertTrue(secureField.waitForExistence(timeout: 3))

        // Tap the toggle's right side where the switch control lives
        tapToggle("showPasswordToggle")

        // After toggling, secure fields become regular text fields
        let textField = app.textFields.matching(identifier: "passwordField").firstMatch
        XCTAssertTrue(textField.waitForExistence(timeout: 3))
    }

    func testCreateVaultPasswordTooShort() {
        app.buttons["Create New Vault"].tap()
        enableShowPassword()

        let vaultNameField = app.textFields["My Vault"]
        XCTAssertTrue(vaultNameField.waitForExistence(timeout: 3))
        vaultNameField.tap()
        vaultNameField.typeText("TestVault")

        let passwordField = app.textFields.matching(identifier: "passwordField").firstMatch
        passwordField.tap()
        passwordField.typeText("short")

        // Tap elsewhere to dismiss keyboard
        app.staticTexts["Vault Name"].tap()

        XCTAssertFalse(app.buttons["Create Vault"].isEnabled)
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'too short'")).firstMatch.exists)
    }

    func testCreateVaultPasswordMismatch() {
        app.buttons["Create New Vault"].tap()
        enableShowPassword()

        let vaultNameField = app.textFields["My Vault"]
        XCTAssertTrue(vaultNameField.waitForExistence(timeout: 3))
        vaultNameField.tap()
        vaultNameField.typeText("TestVault")

        let passwordField = app.textFields.matching(identifier: "passwordField").firstMatch
        passwordField.tap()
        passwordField.typeText("password123")

        let confirmField = app.textFields.matching(identifier: "confirmPasswordField").firstMatch
        confirmField.tap()
        confirmField.typeText("different123")

        app.staticTexts["Vault Name"].tap()

        XCTAssertFalse(app.buttons["Create Vault"].isEnabled)
        XCTAssertTrue(app.staticTexts["Passwords do not match"].exists)
    }

    func testCreateVaultValidFormEnablesButton() {
        app.buttons["Create New Vault"].tap()
        enableShowPassword()

        let vaultNameField = app.textFields["My Vault"]
        XCTAssertTrue(vaultNameField.waitForExistence(timeout: 3))
        vaultNameField.tap()
        vaultNameField.typeText("TestVault")

        let passwordField = app.textFields.matching(identifier: "passwordField").firstMatch
        passwordField.tap()
        passwordField.typeText("password123")

        let confirmField = app.textFields.matching(identifier: "confirmPasswordField").firstMatch
        confirmField.tap()
        confirmField.typeText("password123")

        app.staticTexts["Vault Name"].tap()

        let createButton = app.buttons["Create Vault"]
        let enabled = NSPredicate(format: "isEnabled == true")
        expectation(for: enabled, evaluatedWith: createButton, handler: nil)
        waitForExpectations(timeout: 5)

        XCTAssertTrue(app.staticTexts["Passwords match"].exists)
    }

    func testCreateVaultCancelDismissesSheet() {
        app.buttons["Create New Vault"].tap()
        XCTAssertTrue(app.navigationBars["Create New Vault"].waitForExistence(timeout: 3))

        app.buttons["Cancel"].tap()

        XCTAssertTrue(app.staticTexts["AxiomVault"].waitForExistence(timeout: 3))
    }

    // MARK: - Open Vault Flow

    func testOpenVaultSheetAppears() {
        app.buttons["Open Existing Vault"].tap()

        XCTAssertTrue(app.navigationBars["Open Vault"].waitForExistence(timeout: 3))
    }

    func testOpenVaultHasPasswordField() {
        app.buttons["Open Existing Vault"].tap()

        XCTAssertTrue(app.secureTextFields.matching(identifier: "passwordField").firstMatch.waitForExistence(timeout: 3))
    }

    func testOpenVaultButtonDisabledWithoutPassword() {
        app.buttons["Open Existing Vault"].tap()
        XCTAssertTrue(app.navigationBars["Open Vault"].waitForExistence(timeout: 3))

        // Scroll down to find the Open Vault button (may be below the fold with existing vaults)
        let openButton = app.buttons["Open Vault"]
        if !openButton.exists {
            app.swipeUp()
        }
        XCTAssertTrue(openButton.waitForExistence(timeout: 3))
        XCTAssertFalse(openButton.isEnabled)
    }

    func testOpenVaultCancelDismissesSheet() {
        app.buttons["Open Existing Vault"].tap()
        XCTAssertTrue(app.navigationBars["Open Vault"].waitForExistence(timeout: 3))

        app.buttons["Cancel"].tap()

        XCTAssertTrue(app.staticTexts["AxiomVault"].waitForExistence(timeout: 3))
    }

    func testOpenVaultShowPasswordToggle() {
        app.buttons["Open Existing Vault"].tap()
        XCTAssertTrue(app.navigationBars["Open Vault"].waitForExistence(timeout: 3))

        // Scroll to make sure password section is visible
        let secureField = app.secureTextFields.matching(identifier: "passwordField").firstMatch
        if !secureField.exists {
            app.swipeUp()
        }
        XCTAssertTrue(secureField.waitForExistence(timeout: 3))

        tapToggle("showPasswordToggle")

        let textField = app.textFields.matching(identifier: "passwordField").firstMatch
        XCTAssertTrue(textField.waitForExistence(timeout: 3))
    }

    // MARK: - Full Vault Lifecycle (Create → Browse → Close)

    func testCreateAndBrowseVault() {
        createAndOpenTestVault()

        // Should show the breadcrumb with Root
        XCTAssertTrue(app.buttons["Root"].exists)

        // Close button should be visible
        XCTAssertTrue(app.buttons["Close"].exists)
    }

    func testCloseVaultReturnsToLanding() {
        createAndOpenTestVault()

        app.buttons["Close"].tap()

        XCTAssertTrue(app.staticTexts["AxiomVault"].waitForExistence(timeout: 5))
    }

    // MARK: - Vault Browser Actions

    func testVaultBrowserMenuItems() {
        createAndOpenTestVault()

        app.buttons.matching(identifier: "moreMenu").firstMatch.tap()

        XCTAssertTrue(app.buttons["Add File"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.buttons["New Folder"].exists)
        XCTAssertTrue(app.buttons["Vault Info"].exists)
        XCTAssertTrue(app.buttons["Change Password"].exists)
        XCTAssertTrue(app.buttons["Refresh"].exists)
    }

    func testCreateDirectoryFlow() {
        createAndOpenTestVault()

        app.buttons.matching(identifier: "moreMenu").firstMatch.tap()
        // Use firstMatch to avoid ambiguity with the sheet's nav bar title
        app.buttons["New Folder"].firstMatch.tap()

        XCTAssertTrue(app.navigationBars["New Folder"].waitForExistence(timeout: 3))

        let nameField = app.textFields["New Folder"]
        XCTAssertTrue(nameField.exists)

        let createButton = app.buttons["Create Folder"]
        XCTAssertFalse(createButton.isEnabled)

        nameField.tap()
        nameField.typeText("TestFolder")

        XCTAssertTrue(createButton.isEnabled)

        createButton.tap()

        XCTAssertTrue(app.staticTexts["TestFolder"].waitForExistence(timeout: 5))
    }

    func testVaultInfoSheet() {
        createAndOpenTestVault()

        app.buttons.matching(identifier: "moreMenu").firstMatch.tap()
        app.buttons["Vault Info"].tap()

        XCTAssertTrue(app.navigationBars["Vault Info"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.staticTexts["Vault ID"].exists)
        XCTAssertTrue(app.staticTexts["Files"].exists)
        XCTAssertTrue(app.staticTexts["Cache Size"].exists)

        app.buttons["Done"].tap()
    }

    func testChangePasswordSheet() {
        createAndOpenTestVault()

        app.buttons.matching(identifier: "moreMenu").firstMatch.tap()
        app.buttons["Change Password"].tap()

        XCTAssertTrue(app.navigationBars["Change Password"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.secureTextFields["Current Password"].exists)
        XCTAssertTrue(app.secureTextFields["New Password"].exists)
        XCTAssertTrue(app.secureTextFields["Confirm Password"].exists)

        let changeButton = app.buttons["Change Password"]
        XCTAssertFalse(changeButton.isEnabled)

        app.buttons["Cancel"].tap()
    }

    func testNavigateIntoFolderAndBack() {
        createAndOpenTestVault()

        // Create a folder
        app.buttons.matching(identifier: "moreMenu").firstMatch.tap()
        app.buttons["New Folder"].firstMatch.tap()
        let nameField = app.textFields["New Folder"]
        XCTAssertTrue(nameField.waitForExistence(timeout: 3))
        nameField.tap()
        nameField.typeText("SubFolder")
        app.buttons["Create Folder"].tap()

        // Tap into the folder
        let folder = app.staticTexts["SubFolder"]
        XCTAssertTrue(folder.waitForExistence(timeout: 5))
        folder.tap()

        XCTAssertTrue(app.staticTexts["This folder is empty"].waitForExistence(timeout: 5))

        // Navigate back via breadcrumb
        let rootButton = app.buttons["Root"]
        XCTAssertTrue(rootButton.exists)
        rootButton.tap()

        XCTAssertTrue(app.staticTexts["SubFolder"].waitForExistence(timeout: 5))
    }

    // MARK: - Sync Provider Settings

    func testSyncSettingsSheetAppears() {
        createAndOpenTestVault()

        app.buttons.matching(identifier: "moreMenu").firstMatch.tap()
        app.buttons["Sync Settings"].tap()

        XCTAssertTrue(app.navigationBars["Sync Settings"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.staticTexts["Sync Provider"].exists)
    }

    func testSyncProviderShowsAllOptions() {
        openSyncSettings()

        XCTAssertTrue(app.staticTexts["None"].exists)
        XCTAssertTrue(app.staticTexts["iCloud Drive"].exists)
        XCTAssertTrue(app.staticTexts["Google Drive"].exists)
        XCTAssertTrue(app.staticTexts["WebDAV"].exists)
    }

    func testSelectICloudProviderEnablesControls() {
        openSyncSettings()

        // Select iCloud Drive
        let icloudRow = app.otherElements.matching(identifier: "syncProvider_icloud").firstMatch
        XCTAssertTrue(icloudRow.waitForExistence(timeout: 3))
        icloudRow.tap()

        // Verify availability message updates (no longer says "Select a sync provider")
        XCTAssertFalse(app.staticTexts["Select a sync provider above to enable cloud sync."].exists)
    }

    func testSelectGoogleDriveShowsSetupNotice() {
        openSyncSettings()

        let googleRow = app.otherElements.matching(identifier: "syncProvider_google-drive").firstMatch
        XCTAssertTrue(googleRow.waitForExistence(timeout: 3))
        googleRow.tap()

        // Footer should mention additional setup
        let setupText = app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'requires additional setup'")).firstMatch
        XCTAssertTrue(setupText.waitForExistence(timeout: 3))
    }

    func testSelectNoneDisablesControls() {
        openSyncSettings()

        // First select iCloud to enable controls
        let icloudRow = app.otherElements.matching(identifier: "syncProvider_icloud").firstMatch
        XCTAssertTrue(icloudRow.waitForExistence(timeout: 3))
        icloudRow.tap()

        // Then select None to disable
        let noneRow = app.otherElements.matching(identifier: "syncProvider_none").firstMatch
        noneRow.tap()

        // Should show the "Select a provider" message again
        let selectMessage = app.staticTexts["Select a sync provider above to enable cloud sync."]
        XCTAssertTrue(selectMessage.waitForExistence(timeout: 3))
    }

    func testSyncProviderSelectionPersists() {
        openSyncSettings()

        // Select iCloud Drive
        let icloudRow = app.otherElements.matching(identifier: "syncProvider_icloud").firstMatch
        XCTAssertTrue(icloudRow.waitForExistence(timeout: 3))
        icloudRow.tap()

        // Dismiss sync settings
        app.buttons["Done"].tap()

        // Reopen sync settings
        app.buttons.matching(identifier: "moreMenu").firstMatch.tap()
        app.buttons["Sync Settings"].tap()
        XCTAssertTrue(app.navigationBars["Sync Settings"].waitForExistence(timeout: 3))

        // iCloud should still show the checkmark (availability message should not be the "select" prompt)
        XCTAssertFalse(app.staticTexts["Select a sync provider above to enable cloud sync."].exists)
    }

    func testWebDAVShowsSetupNotice() {
        openSyncSettings()

        let webdavRow = app.otherElements.matching(identifier: "syncProvider_webdav").firstMatch
        XCTAssertTrue(webdavRow.waitForExistence(timeout: 3))
        webdavRow.tap()

        let setupText = app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'requires additional setup'")).firstMatch
        XCTAssertTrue(setupText.waitForExistence(timeout: 3))
    }

    // MARK: - Helpers

    /// Opens Sync Settings from the vault browser menu.
    private func openSyncSettings() {
        createAndOpenTestVault()

        app.buttons.matching(identifier: "moreMenu").firstMatch.tap()
        app.buttons["Sync Settings"].tap()

        XCTAssertTrue(app.navigationBars["Sync Settings"].waitForExistence(timeout: 3))
    }

    // MARK: - Existing Helpers

    /// Tap a SwiftUI Toggle by its accessibility identifier.
    /// Taps the right side of the switch where the control lives.
    private func tapToggle(_ identifier: String) {
        let toggle = app.switches.matching(identifier: identifier).firstMatch
        if toggle.waitForExistence(timeout: 3) {
            // Tap the right side where the actual switch control is
            toggle.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)).tap()
        }
    }

    /// Enables "Show Password" in the current Create Vault sheet,
    /// converting SecureFields to TextFields for reliable typing.
    private func enableShowPassword() {
        let secureField = app.secureTextFields.matching(identifier: "passwordField").firstMatch
        XCTAssertTrue(secureField.waitForExistence(timeout: 3))

        tapToggle("showPasswordToggle")

        // Wait for text field to appear (confirms toggle worked)
        let textField = app.textFields.matching(identifier: "passwordField").firstMatch
        XCTAssertTrue(textField.waitForExistence(timeout: 3))
    }

    /// Creates a vault and navigates to the browser view.
    private func createAndOpenTestVault() {
        app.buttons["Create New Vault"].tap()
        enableShowPassword()

        let vaultNameField = app.textFields["My Vault"]
        XCTAssertTrue(vaultNameField.waitForExistence(timeout: 3))
        vaultNameField.tap()
        vaultNameField.typeText("UITest-\(UUID().uuidString.prefix(6))")

        let passwordField = app.textFields.matching(identifier: "passwordField").firstMatch
        passwordField.tap()
        passwordField.typeText("testpassword123")

        let confirmField = app.textFields.matching(identifier: "confirmPasswordField").firstMatch
        confirmField.tap()
        confirmField.typeText("testpassword123")

        // Dismiss keyboard
        app.staticTexts["Vault Name"].tap()

        let createButton = app.buttons["Create Vault"]
        let enabled = NSPredicate(format: "isEnabled == true")
        expectation(for: enabled, evaluatedWith: createButton, handler: nil)
        waitForExpectations(timeout: 5)

        createButton.tap()

        // Wait for vault browser
        let emptyFolder = app.staticTexts["This folder is empty"]
        let rootBreadcrumb = app.buttons["Root"]
        let browserLoaded = NSPredicate { _, _ in
            emptyFolder.exists || rootBreadcrumb.exists
        }
        expectation(for: browserLoaded, evaluatedWith: nil, handler: nil)
        waitForExpectations(timeout: 15)
    }
}
