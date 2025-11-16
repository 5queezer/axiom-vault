package com.axiomvault.android.di

import android.content.Context
import com.axiomvault.android.core.VaultCore
import com.axiomvault.android.services.BiometricAuthService
import com.axiomvault.android.services.GoogleDriveService
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.android.qualifiers.ApplicationContext
import dagger.hilt.components.SingletonComponent
import javax.inject.Singleton

@Module
@InstallIn(SingletonComponent::class)
object AppModule {

    @Provides
    @Singleton
    fun provideVaultCore(): VaultCore {
        return VaultCore()
    }

    @Provides
    @Singleton
    fun provideBiometricAuthService(
        @ApplicationContext context: Context
    ): BiometricAuthService {
        return BiometricAuthService(context)
    }

    @Provides
    @Singleton
    fun provideGoogleDriveService(
        @ApplicationContext context: Context
    ): GoogleDriveService {
        return GoogleDriveService(context)
    }
}
