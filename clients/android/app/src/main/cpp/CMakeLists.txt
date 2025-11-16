cmake_minimum_required(VERSION 3.22.1)

project("axiom_vault_jni")

# Find the pre-built Rust library
add_library(axiom_vault SHARED IMPORTED)
set_target_properties(axiom_vault PROPERTIES
    IMPORTED_LOCATION ${CMAKE_SOURCE_DIR}/../jniLibs/${ANDROID_ABI}/libaxiom_vault.so
)

# Create the JNI wrapper library
add_library(${CMAKE_PROJECT_NAME} SHARED
    axiom_vault_jni.cpp
)

# Include headers for the Rust FFI
target_include_directories(${CMAKE_PROJECT_NAME} PRIVATE
    ${CMAKE_SOURCE_DIR}/include
)

# Link against the Rust library and Android log
target_link_libraries(${CMAKE_PROJECT_NAME}
    axiom_vault
    android
    log
)

# Set C++ standard
set_property(TARGET ${CMAKE_PROJECT_NAME} PROPERTY CXX_STANDARD 17)
