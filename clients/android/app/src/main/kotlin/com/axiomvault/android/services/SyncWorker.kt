package com.axiomvault.android.services

import android.content.Context
import androidx.hilt.work.HiltWorker
import androidx.work.*
import dagger.assisted.Assisted
import dagger.assisted.AssistedInject
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.util.concurrent.TimeUnit

@HiltWorker
class SyncWorker @AssistedInject constructor(
    @Assisted context: Context,
    @Assisted workerParams: WorkerParameters,
    private val googleDriveService: GoogleDriveService
) : CoroutineWorker(context, workerParams) {

    companion object {
        const val WORK_NAME = "vault_sync_work"
        const val TAG = "SyncWorker"

        private const val KEY_VAULT_PATH = "vault_path"

        fun createInputData(vaultPath: String): Data {
            return Data.Builder()
                .putString(KEY_VAULT_PATH, vaultPath)
                .build()
        }

        fun schedulePeriodicSync(
            context: Context,
            vaultPath: String,
            intervalMinutes: Long = 15
        ) {
            val constraints = Constraints.Builder()
                .setRequiredNetworkType(NetworkType.CONNECTED)
                .setRequiresBatteryNotLow(true)
                .build()

            val syncRequest = PeriodicWorkRequestBuilder<SyncWorker>(
                intervalMinutes,
                TimeUnit.MINUTES
            )
                .setConstraints(constraints)
                .setInputData(createInputData(vaultPath))
                .addTag(TAG)
                .setBackoffCriteria(
                    BackoffPolicy.EXPONENTIAL,
                    WorkRequest.MIN_BACKOFF_MILLIS,
                    TimeUnit.MILLISECONDS
                )
                .build()

            WorkManager.getInstance(context)
                .enqueueUniquePeriodicWork(
                    WORK_NAME,
                    ExistingPeriodicWorkPolicy.UPDATE,
                    syncRequest
                )
        }

        fun scheduleOneTimeSync(context: Context, vaultPath: String) {
            val constraints = Constraints.Builder()
                .setRequiredNetworkType(NetworkType.CONNECTED)
                .build()

            val syncRequest = OneTimeWorkRequestBuilder<SyncWorker>()
                .setConstraints(constraints)
                .setInputData(createInputData(vaultPath))
                .addTag(TAG)
                .setBackoffCriteria(
                    BackoffPolicy.EXPONENTIAL,
                    WorkRequest.MIN_BACKOFF_MILLIS,
                    TimeUnit.MILLISECONDS
                )
                .build()

            WorkManager.getInstance(context)
                .enqueue(syncRequest)
        }

        fun cancelSync(context: Context) {
            WorkManager.getInstance(context)
                .cancelUniqueWork(WORK_NAME)
        }
    }

    override suspend fun doWork(): Result = withContext(Dispatchers.IO) {
        val vaultPath = inputData.getString(KEY_VAULT_PATH)
            ?: return@withContext Result.failure()

        return@withContext try {
            // Check if signed in to Google Drive
            if (!googleDriveService.state.value.isSignedIn) {
                return@withContext Result.failure()
            }

            // Perform sync
            val syncResult = googleDriveService.syncVault(vaultPath)

            if (syncResult.isSuccess) {
                Result.success()
            } else {
                // Retry on failure
                if (runAttemptCount < 3) {
                    Result.retry()
                } else {
                    Result.failure()
                }
            }
        } catch (e: Exception) {
            if (runAttemptCount < 3) {
                Result.retry()
            } else {
                Result.failure()
            }
        }
    }
}
