package com.axiomvault.android.services

import android.content.Context
import android.content.Intent
import com.google.android.gms.auth.api.signin.GoogleSignIn
import com.google.android.gms.auth.api.signin.GoogleSignInAccount
import com.google.android.gms.auth.api.signin.GoogleSignInClient
import com.google.android.gms.auth.api.signin.GoogleSignInOptions
import com.google.android.gms.common.api.Scope
import com.google.api.client.googleapis.extensions.android.gms.auth.GoogleAccountCredential
import com.google.api.client.http.javanet.NetHttpTransport
import com.google.api.client.json.gson.GsonFactory
import com.google.api.services.drive.Drive
import com.google.api.services.drive.DriveScopes
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.withContext
import javax.inject.Inject
import javax.inject.Singleton

data class GoogleDriveState(
    val isSignedIn: Boolean = false,
    val accountName: String? = null,
    val lastSyncTime: Long? = null
)

@Singleton
class GoogleDriveService @Inject constructor(
    private val context: Context
) {
    private var googleSignInClient: GoogleSignInClient
    private var driveService: Drive? = null

    private val _state = MutableStateFlow(GoogleDriveState())
    val state: StateFlow<GoogleDriveState> = _state.asStateFlow()

    init {
        val signInOptions = GoogleSignInOptions.Builder(GoogleSignInOptions.DEFAULT_SIGN_IN)
            .requestEmail()
            .requestScopes(Scope(DriveScopes.DRIVE_FILE))
            .build()

        googleSignInClient = GoogleSignIn.getClient(context, signInOptions)

        // Check if already signed in
        val account = GoogleSignIn.getLastSignedInAccount(context)
        if (account != null) {
            initializeDriveService(account)
        }
    }

    fun getSignInIntent(): Intent = googleSignInClient.signInIntent

    fun handleSignInResult(account: GoogleSignInAccount?) {
        if (account != null) {
            initializeDriveService(account)
        }
    }

    private fun initializeDriveService(account: GoogleSignInAccount) {
        val credential = GoogleAccountCredential.usingOAuth2(
            context,
            listOf(DriveScopes.DRIVE_FILE)
        )
        credential.selectedAccount = account.account

        driveService = Drive.Builder(
            NetHttpTransport(),
            GsonFactory.getDefaultInstance(),
            credential
        )
            .setApplicationName("AxiomVault")
            .build()

        _state.value = GoogleDriveState(
            isSignedIn = true,
            accountName = account.email
        )
    }

    suspend fun signOut() = withContext(Dispatchers.IO) {
        googleSignInClient.signOut().addOnCompleteListener {
            driveService = null
            _state.value = GoogleDriveState()
        }
    }

    suspend fun syncVault(localVaultPath: String): Result<Unit> = withContext(Dispatchers.IO) {
        val service = driveService ?: return@withContext Result.failure(
            Exception("Not signed in to Google Drive")
        )

        try {
            // TODO: Implement actual sync logic with the vault
            // This would involve:
            // 1. List files in Google Drive
            // 2. Compare with local vault
            // 3. Upload/download changes
            // 4. Handle conflicts

            _state.value = _state.value.copy(
                lastSyncTime = System.currentTimeMillis()
            )

            Result.success(Unit)
        } catch (e: Exception) {
            Result.failure(e)
        }
    }

    suspend fun uploadFile(
        localFilePath: String,
        driveFileName: String,
        mimeType: String = "application/octet-stream"
    ): Result<String> = withContext(Dispatchers.IO) {
        val service = driveService ?: return@withContext Result.failure(
            Exception("Not signed in to Google Drive")
        )

        try {
            val fileMetadata = com.google.api.services.drive.model.File().apply {
                name = driveFileName
            }

            val file = java.io.File(localFilePath)
            val mediaContent = com.google.api.client.http.FileContent(mimeType, file)

            val uploadedFile = service.files().create(fileMetadata, mediaContent)
                .setFields("id")
                .execute()

            Result.success(uploadedFile.id)
        } catch (e: Exception) {
            Result.failure(e)
        }
    }

    suspend fun downloadFile(
        driveFileId: String,
        localFilePath: String
    ): Result<Unit> = withContext(Dispatchers.IO) {
        val service = driveService ?: return@withContext Result.failure(
            Exception("Not signed in to Google Drive")
        )

        try {
            val outputStream = java.io.FileOutputStream(localFilePath)
            service.files().get(driveFileId).executeMediaAndDownloadTo(outputStream)
            outputStream.close()

            Result.success(Unit)
        } catch (e: Exception) {
            Result.failure(e)
        }
    }

    suspend fun listFiles(): Result<List<String>> = withContext(Dispatchers.IO) {
        val service = driveService ?: return@withContext Result.failure(
            Exception("Not signed in to Google Drive")
        )

        try {
            val result = service.files().list()
                .setSpaces("drive")
                .setFields("files(id, name)")
                .execute()

            val fileNames = result.files.map { it.name }
            Result.success(fileNames)
        } catch (e: Exception) {
            Result.failure(e)
        }
    }
}
