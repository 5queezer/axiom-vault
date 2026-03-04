# App Store Metadata — AxiomVault

## App Identity

| Field | Value |
|-------|-------|
| **App Name** | AxiomVault |
| **Subtitle** | Encrypted Offline Vault |
| **Bundle ID** | com.axiomvault.ios |
| **Primary Category** | Productivity |
| **Secondary Category** | Utilities |

---

## Description

**AxiomVault** is a privacy-first encrypted vault for storing your most sensitive files — completely offline, with no cloud dependency by default.

Your data is encrypted on-device using **XChaCha20-Poly1305**, one of the strongest authenticated encryption algorithms available. Only you hold the key. No accounts. No servers. No tracking.

### Why AxiomVault?

- 🔐 **Military-grade encryption** — XChaCha20-Poly1305 with Argon2 key derivation
- 📵 **Fully offline** — works without an internet connection; your data never leaves your device unless you choose
- 🚫 **Zero knowledge** — we have no servers and no access to your data. Ever.
- 🔑 **Biometric unlock** — Face ID / Touch ID for quick, secure vault access
- ☁️ **Optional Google Drive sync** — encrypted sync; plaintext never leaves your device
- 📁 **File vault** — store documents, photos, notes, and any file type
- 🗂️ **Folder organisation** — create a nested folder structure inside your vault

### How it works

AxiomVault creates an encrypted container on your device. Every file you add is encrypted before storage. When you unlock with your password or Face ID, files are decrypted in memory only — they never exist unencrypted on disk outside the vault.

Google Drive sync (optional) uploads already-encrypted vault chunks. Even if your Drive account were compromised, your data remains unreadable without your password.

### Privacy commitment

AxiomVault collects **zero** data. There are no analytics, no crash reporters phoning home, no third-party SDKs that track you. The developer has no visibility into what you store or how you use the app.

---

## Keywords

`encrypted vault`, `secure storage`, `offline vault`, `file encryption`, `privacy`, `password manager alternative`, `encrypted files`, `secure documents`, `XChaCha20`, `zero knowledge`

> Maximum 100 characters for App Store field — suggested:
> `encrypted vault,secure storage,offline,file encryption,privacy,zero knowledge,secure documents`

---

## Screenshots & Preview

Recommended screenshot sets (6.5" + 5.5"):
1. Vault locked screen (password + Face ID unlock button)
2. Vault browser — folder list
3. Add file / encryption in progress
4. Settings / privacy summary ("No data collected")
5. Google Drive sync toggle (optional)

---

## Privacy Nutrition Labels

> **Data Not Collected** — AxiomVault does not collect any data from users.

| Category | Collected? | Notes |
|----------|-----------|-------|
| Contact Info | ❌ No | |
| Health & Fitness | ❌ No | |
| Financial Info | ❌ No | |
| Location | ❌ No | |
| Sensitive Info | ❌ No | |
| Contacts | ❌ No | |
| User Content | ❌ No | Files stay on-device; developer has no access |
| Browsing History | ❌ No | |
| Search History | ❌ No | |
| Identifiers | ❌ No | |
| Usage Data | ❌ No | No analytics |
| Diagnostics | ❌ No | No crash reporting to developer |
| Other Data | ❌ No | |

**Answer to Apple's privacy questionnaire:** "This app does not collect data from users."

---

## Content Rating

| Rating | 4+ |
|--------|-----|
| Reason | No objectionable content; no user-generated public content; no social features |

---

## Export Compliance

| Field | Value |
|-------|-------|
| Uses encryption | **Yes** |
| Encryption standard | XChaCha20-Poly1305 (IETF RFC 8439 variant) + Argon2id KDF |
| Qualifies for ENC exception | **Yes** — exempt under EAR §742.15(b) (standard encryption for data protection) |
| `ITSAppUsesNonExemptEncryption` in Info.plist | `true` |
| ERN (Encryption Registration Number) | *Obtain and record here before first submission* |

> **Note:** Because the app uses non-exempt encryption (XChaCha20), you must answer
> "Yes" to the encryption questions in App Store Connect and may need to file an
> Annual Self-Classification Report with the U.S. Bureau of Industry and Security (BIS).
> Consult your legal team for jurisdiction-specific requirements.

---

## Support & Marketing URLs

| Field | URL |
|-------|-----|
| Support URL | https://github.com/5queezer/axiom-vault/issues |
| Marketing URL | https://github.com/5queezer/axiom-vault |
| Privacy Policy URL | *(add before submission — required field)* |

> A Privacy Policy URL is **mandatory** for App Store submission. The policy should
> state that no data is collected and explain the on-device encryption model.

---

## Release Notes (v0.1.0 — Initial Release)

```
AxiomVault is now available on the App Store!

• Create encrypted vaults secured with XChaCha20-Poly1305
• Unlock with Face ID or your master password
• Organise files in nested folders within your vault
• Optional Google Drive sync (encrypted — plaintext never leaves your device)
• Zero data collection — your files, your keys, your privacy
```
