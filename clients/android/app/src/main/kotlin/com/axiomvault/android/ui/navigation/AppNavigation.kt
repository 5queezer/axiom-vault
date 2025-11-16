package com.axiomvault.android.ui.navigation

import androidx.compose.runtime.Composable
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import com.axiomvault.android.ui.screens.CreateVaultScreen
import com.axiomvault.android.ui.screens.HomeScreen
import com.axiomvault.android.ui.screens.OpenVaultScreen
import com.axiomvault.android.ui.screens.VaultBrowserScreen

sealed class Screen(val route: String) {
    data object Home : Screen("home")
    data object CreateVault : Screen("create_vault")
    data object OpenVault : Screen("open_vault")
    data object VaultBrowser : Screen("vault_browser")
}

@Composable
fun AppNavigation(
    navController: NavHostController = rememberNavController()
) {
    NavHost(
        navController = navController,
        startDestination = Screen.Home.route
    ) {
        composable(Screen.Home.route) {
            HomeScreen(
                onCreateVault = { navController.navigate(Screen.CreateVault.route) },
                onOpenVault = { navController.navigate(Screen.OpenVault.route) },
                onVaultOpened = { navController.navigate(Screen.VaultBrowser.route) }
            )
        }

        composable(Screen.CreateVault.route) {
            CreateVaultScreen(
                onVaultCreated = {
                    navController.navigate(Screen.VaultBrowser.route) {
                        popUpTo(Screen.Home.route)
                    }
                },
                onBack = { navController.popBackStack() }
            )
        }

        composable(Screen.OpenVault.route) {
            OpenVaultScreen(
                onVaultOpened = {
                    navController.navigate(Screen.VaultBrowser.route) {
                        popUpTo(Screen.Home.route)
                    }
                },
                onBack = { navController.popBackStack() }
            )
        }

        composable(Screen.VaultBrowser.route) {
            VaultBrowserScreen(
                onCloseVault = {
                    navController.navigate(Screen.Home.route) {
                        popUpTo(Screen.Home.route) { inclusive = true }
                    }
                }
            )
        }
    }
}
