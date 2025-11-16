package com.axiomvault.android.ui.screens

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Fingerprint
import androidx.compose.material.icons.filled.Visibility
import androidx.compose.material.icons.filled.VisibilityOff
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.fragment.app.FragmentActivity
import androidx.hilt.navigation.compose.hiltViewModel
import com.axiomvault.android.models.VaultViewModel
import com.axiomvault.android.services.BiometricAuthService
import com.axiomvault.android.services.BiometricResult
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun OpenVaultScreen(
    viewModel: VaultViewModel = hiltViewModel(),
    onVaultOpened: () -> Unit,
    onBack: () -> Unit
) {
    val state by viewModel.state.collectAsState()
    val context = LocalContext.current
    val scope = rememberCoroutineScope()

    var vaultPath by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var passwordVisible by remember { mutableStateOf(false) }
    var validationError by remember { mutableStateOf<String?>(null) }

    // Get list of available vaults
    val availableVaults by remember {
        mutableStateOf(
            context.filesDir.listFiles()
                ?.filter { it.extension == "vault" }
                ?.map { it.name.removeSuffix(".vault") }
                ?: emptyList()
        )
    }
    var selectedVault by remember { mutableStateOf(availableVaults.firstOrNull() ?: "") }
    var expanded by remember { mutableStateOf(false) }

    val biometricService = remember { BiometricAuthService(context) }
    val isBiometricAvailable = remember { biometricService.isBiometricAvailable() }

    LaunchedEffect(state.isVaultOpen) {
        if (state.isVaultOpen) {
            onVaultOpened()
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Open Vault") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(
                            imageVector = Icons.AutoMirrored.Filled.ArrowBack,
                            contentDescription = "Back"
                        )
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.primary,
                    titleContentColor = MaterialTheme.colorScheme.onPrimary,
                    navigationIconContentColor = MaterialTheme.colorScheme.onPrimary
                )
            )
        }
    ) { paddingValues ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(paddingValues)
                .padding(24.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp)
        ) {
            if (availableVaults.isNotEmpty()) {
                ExposedDropdownMenuBox(
                    expanded = expanded,
                    onExpandedChange = { expanded = !expanded }
                ) {
                    OutlinedTextField(
                        value = selectedVault,
                        onValueChange = {},
                        readOnly = true,
                        label = { Text("Select Vault") },
                        trailingIcon = { ExposedDropdownMenuDefaults.TrailingIcon(expanded = expanded) },
                        modifier = Modifier
                            .menuAnchor()
                            .fillMaxWidth()
                    )

                    ExposedDropdownMenu(
                        expanded = expanded,
                        onDismissRequest = { expanded = false }
                    ) {
                        availableVaults.forEach { vault ->
                            DropdownMenuItem(
                                text = { Text(vault) },
                                onClick = {
                                    selectedVault = vault
                                    expanded = false
                                }
                            )
                        }
                    }
                }
            } else {
                OutlinedTextField(
                    value = vaultPath,
                    onValueChange = {
                        vaultPath = it
                        validationError = null
                    },
                    label = { Text("Vault Path") },
                    modifier = Modifier.fillMaxWidth(),
                    singleLine = true,
                    placeholder = { Text("Path to vault file") }
                )
            }

            OutlinedTextField(
                value = password,
                onValueChange = {
                    password = it
                    validationError = null
                },
                label = { Text("Password") },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
                visualTransformation = if (passwordVisible) {
                    VisualTransformation.None
                } else {
                    PasswordVisualTransformation()
                },
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                trailingIcon = {
                    IconButton(onClick = { passwordVisible = !passwordVisible }) {
                        Icon(
                            imageVector = if (passwordVisible) {
                                Icons.Default.VisibilityOff
                            } else {
                                Icons.Default.Visibility
                            },
                            contentDescription = if (passwordVisible) {
                                "Hide password"
                            } else {
                                "Show password"
                            }
                        )
                    }
                }
            )

            validationError?.let { error ->
                Text(
                    text = error,
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodySmall
                )
            }

            state.error?.let { error ->
                Text(
                    text = error,
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodySmall
                )
            }

            if (isBiometricAvailable) {
                OutlinedButton(
                    onClick = {
                        scope.launch {
                            val activity = context as? FragmentActivity
                            if (activity != null) {
                                when (biometricService.authenticate(activity)) {
                                    is BiometricResult.Success -> {
                                        // TODO: Retrieve stored password for vault
                                        // For now, just show a message
                                    }
                                    is BiometricResult.Error -> {
                                        validationError = "Biometric authentication failed"
                                    }
                                    is BiometricResult.Cancelled -> {
                                        // User cancelled, do nothing
                                    }
                                }
                            }
                        }
                    },
                    modifier = Modifier.fillMaxWidth()
                ) {
                    Icon(
                        imageVector = Icons.Default.Fingerprint,
                        contentDescription = null,
                        modifier = Modifier.size(24.dp)
                    )
                    Spacer(modifier = Modifier.width(8.dp))
                    Text("Use Biometric")
                }
            }

            Spacer(modifier = Modifier.weight(1f))

            Button(
                onClick = {
                    val finalPath = if (availableVaults.isNotEmpty()) {
                        "${context.filesDir.absolutePath}/$selectedVault.vault"
                    } else {
                        vaultPath
                    }

                    if (finalPath.isEmpty()) {
                        validationError = "Please select or enter a vault path"
                    } else if (password.isEmpty()) {
                        validationError = "Password cannot be empty"
                    } else {
                        viewModel.openVault(finalPath, password)
                    }
                },
                modifier = Modifier
                    .fillMaxWidth()
                    .height(56.dp),
                enabled = !state.isLoading && password.isNotEmpty() &&
                         (selectedVault.isNotEmpty() || vaultPath.isNotEmpty())
            ) {
                if (state.isLoading) {
                    CircularProgressIndicator(
                        modifier = Modifier.size(24.dp),
                        color = MaterialTheme.colorScheme.onPrimary
                    )
                } else {
                    Text("Open Vault")
                }
            }
        }
    }
}
