#include <jni.h>
#include <string>
#include <android/log.h>

// Forward declarations for Rust FFI functions (from axiom_ffi.h)
extern "C" {
    int axiom_init();
    const char* axiom_version();
    void* axiom_vault_create(const char* path, const char* password);
    void* axiom_vault_open(const char* path, const char* password);
    int axiom_vault_close(void* handle);
    void* axiom_vault_info(const void* handle);
    char* axiom_vault_list(const void* handle, const char* path);
    int axiom_vault_add_file(const void* handle, const char* local_path, const char* vault_path);
    int axiom_vault_extract_file(const void* handle, const char* vault_path, const char* local_path);
    int axiom_vault_mkdir(const void* handle, const char* vault_path);
    int axiom_vault_remove(const void* handle, const char* vault_path);
    int axiom_vault_change_password(const void* handle, const char* old_password, const char* new_password);
    char* axiom_last_error();
    void axiom_string_free(char* s);
    void axiom_vault_info_free(void* info);

    // FFI Vault Info structure
    typedef struct {
        const char* vault_id;
        const char* root_path;
        int file_count;
        long long total_size;
        int version;
    } FFIVaultInfo;
}

#define LOG_TAG "AxiomVaultJNI"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

extern "C" {

JNIEXPORT jint JNICALL
Java_com_axiomvault_android_core_VaultCore_nativeInit(JNIEnv *env, jobject thiz) {
    LOGI("Initializing AxiomVault FFI");
    return axiom_init();
}

JNIEXPORT jstring JNICALL
Java_com_axiomvault_android_core_VaultCore_nativeVersion(JNIEnv *env, jobject thiz) {
    const char* version = axiom_version();
    return env->NewStringUTF(version ? version : "unknown");
}

JNIEXPORT jlong JNICALL
Java_com_axiomvault_android_core_VaultCore_nativeCreateVault(JNIEnv *env, jobject thiz,
                                                               jstring path, jstring password) {
    const char* path_str = env->GetStringUTFChars(path, nullptr);
    const char* pwd_str = env->GetStringUTFChars(password, nullptr);

    LOGI("Creating vault at: %s", path_str);
    void* handle = axiom_vault_create(path_str, pwd_str);

    env->ReleaseStringUTFChars(path, path_str);
    env->ReleaseStringUTFChars(password, pwd_str);

    return reinterpret_cast<jlong>(handle);
}

JNIEXPORT jlong JNICALL
Java_com_axiomvault_android_core_VaultCore_nativeOpenVault(JNIEnv *env, jobject thiz,
                                                             jstring path, jstring password) {
    const char* path_str = env->GetStringUTFChars(path, nullptr);
    const char* pwd_str = env->GetStringUTFChars(password, nullptr);

    LOGI("Opening vault at: %s", path_str);
    void* handle = axiom_vault_open(path_str, pwd_str);

    env->ReleaseStringUTFChars(path, path_str);
    env->ReleaseStringUTFChars(password, pwd_str);

    return reinterpret_cast<jlong>(handle);
}

JNIEXPORT jint JNICALL
Java_com_axiomvault_android_core_VaultCore_nativeCloseVault(JNIEnv *env, jobject thiz,
                                                              jlong handle) {
    LOGI("Closing vault");
    return axiom_vault_close(reinterpret_cast<void*>(handle));
}

JNIEXPORT jstring JNICALL
Java_com_axiomvault_android_core_VaultCore_nativeVaultInfo(JNIEnv *env, jobject thiz,
                                                             jlong handle) {
    FFIVaultInfo* info = reinterpret_cast<FFIVaultInfo*>(
        axiom_vault_info(reinterpret_cast<const void*>(handle))
    );

    if (!info) {
        return nullptr;
    }

    // Convert to JSON string for easier parsing in Kotlin
    char json_buffer[1024];
    snprintf(json_buffer, sizeof(json_buffer),
             R"({"vault_id":"%s","root_path":"%s","file_count":%d,"total_size":%lld,"version":%d})",
             info->vault_id ? info->vault_id : "",
             info->root_path ? info->root_path : "",
             info->file_count,
             info->total_size,
             info->version);

    axiom_vault_info_free(info);

    return env->NewStringUTF(json_buffer);
}

JNIEXPORT jstring JNICALL
Java_com_axiomvault_android_core_VaultCore_nativeVaultList(JNIEnv *env, jobject thiz,
                                                             jlong handle, jstring path) {
    const char* path_str = env->GetStringUTFChars(path, nullptr);

    char* json = axiom_vault_list(reinterpret_cast<const void*>(handle), path_str);

    env->ReleaseStringUTFChars(path, path_str);

    if (!json) {
        return nullptr;
    }

    jstring result = env->NewStringUTF(json);
    axiom_string_free(json);

    return result;
}

JNIEXPORT jint JNICALL
Java_com_axiomvault_android_core_VaultCore_nativeAddFile(JNIEnv *env, jobject thiz,
                                                           jlong handle, jstring local_path,
                                                           jstring vault_path) {
    const char* local_str = env->GetStringUTFChars(local_path, nullptr);
    const char* vault_str = env->GetStringUTFChars(vault_path, nullptr);

    LOGI("Adding file: %s -> %s", local_str, vault_str);
    int result = axiom_vault_add_file(reinterpret_cast<const void*>(handle), local_str, vault_str);

    env->ReleaseStringUTFChars(local_path, local_str);
    env->ReleaseStringUTFChars(vault_path, vault_str);

    return result;
}

JNIEXPORT jint JNICALL
Java_com_axiomvault_android_core_VaultCore_nativeExtractFile(JNIEnv *env, jobject thiz,
                                                               jlong handle, jstring vault_path,
                                                               jstring local_path) {
    const char* vault_str = env->GetStringUTFChars(vault_path, nullptr);
    const char* local_str = env->GetStringUTFChars(local_path, nullptr);

    LOGI("Extracting file: %s -> %s", vault_str, local_str);
    int result = axiom_vault_extract_file(reinterpret_cast<const void*>(handle), vault_str, local_str);

    env->ReleaseStringUTFChars(vault_path, vault_str);
    env->ReleaseStringUTFChars(local_path, local_str);

    return result;
}

JNIEXPORT jint JNICALL
Java_com_axiomvault_android_core_VaultCore_nativeMkdir(JNIEnv *env, jobject thiz,
                                                         jlong handle, jstring vault_path) {
    const char* path_str = env->GetStringUTFChars(vault_path, nullptr);

    LOGI("Creating directory: %s", path_str);
    int result = axiom_vault_mkdir(reinterpret_cast<const void*>(handle), path_str);

    env->ReleaseStringUTFChars(vault_path, path_str);

    return result;
}

JNIEXPORT jint JNICALL
Java_com_axiomvault_android_core_VaultCore_nativeRemove(JNIEnv *env, jobject thiz,
                                                          jlong handle, jstring vault_path) {
    const char* path_str = env->GetStringUTFChars(vault_path, nullptr);

    LOGI("Removing: %s", path_str);
    int result = axiom_vault_remove(reinterpret_cast<const void*>(handle), path_str);

    env->ReleaseStringUTFChars(vault_path, path_str);

    return result;
}

JNIEXPORT jint JNICALL
Java_com_axiomvault_android_core_VaultCore_nativeChangePassword(JNIEnv *env, jobject thiz,
                                                                   jlong handle,
                                                                   jstring old_password,
                                                                   jstring new_password) {
    const char* old_str = env->GetStringUTFChars(old_password, nullptr);
    const char* new_str = env->GetStringUTFChars(new_password, nullptr);

    LOGI("Changing vault password");
    int result = axiom_vault_change_password(reinterpret_cast<const void*>(handle), old_str, new_str);

    env->ReleaseStringUTFChars(old_password, old_str);
    env->ReleaseStringUTFChars(new_password, new_str);

    return result;
}

JNIEXPORT jstring JNICALL
Java_com_axiomvault_android_core_VaultCore_nativeLastError(JNIEnv *env, jobject thiz) {
    char* error = axiom_last_error();
    if (!error) {
        return nullptr;
    }

    jstring result = env->NewStringUTF(error);
    axiom_string_free(error);

    return result;
}

} // extern "C"
