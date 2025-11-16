package com.axiomvault.android.services

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

class BootReceiver : BroadcastReceiver() {

    companion object {
        private const val TAG = "BootReceiver"
    }

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED) {
            Log.i(TAG, "Device booted, re-scheduling sync if configured")

            // Re-schedule periodic sync if it was configured
            // In a real implementation, this would read from preferences
            // to determine if sync was enabled and for which vault

            // For now, we just log that the boot was received
            // The actual re-scheduling would happen when the user opens the app
        }
    }
}
