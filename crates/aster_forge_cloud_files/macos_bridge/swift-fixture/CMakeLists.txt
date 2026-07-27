cmake_minimum_required(VERSION 3.29)

set(CMAKE_OSX_DEPLOYMENT_TARGET "13.0" CACHE STRING "Minimum supported macOS version")

project(AsterForgeCloudFilesMacosFixture LANGUAGES C Swift)

enable_testing()

if(NOT APPLE)
  message(FATAL_ERROR "The macOS File Provider fixture requires Apple SDKs")
endif()

set(CMAKE_XCODE_GENERATE_TOP_LEVEL_PROJECT_ONLY TRUE)
set(CMAKE_Swift_LANGUAGE_VERSION 5)

set(
  ASTER_FORGE_DEVELOPMENT_TEAM
  ""
  CACHE STRING
  "Apple development team used for local signed builds"
)
set(
  ASTER_FORGE_HOST_BUNDLE_ID
  "com.asterforge.cloudfiles.memory-fixture"
  CACHE STRING
  "Host application bundle identifier"
)
set(
  ASTER_FORGE_EXTENSION_BUNDLE_ID
  "com.asterforge.cloudfiles.memory-fixture.file-provider"
  CACHE STRING
  "File Provider extension bundle identifier"
)
set(
  ASTER_FORGE_APP_GROUP_ID
  "group.com.asterforge.cloudfiles.memory-fixture"
  CACHE STRING
  "Shared application group identifier"
)
set(
  ASTER_FORGE_DOMAIN_ID
  "aster-forge-memory-cloud"
  CACHE STRING
  "Synthetic File Provider domain identifier"
)
set(
  ASTER_FORGE_CODE_SIGNING_ALLOWED
  "YES"
  CACHE STRING
  "Set to NO for compile-only CI builds"
)

include("${CMAKE_CURRENT_SOURCE_DIR}/LocalSigning.cmake" OPTIONAL)

get_filename_component(REPOSITORY_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../../../.." ABSOLUTE)
set(MACOS_BRIDGE_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/..")
set(SWIFT_SHELL_ROOT "${MACOS_BRIDGE_ROOT}/swift")
set(GENERATED_ROOT "${CMAKE_CURRENT_BINARY_DIR}/generated")
set(RUST_TARGET_DIR "${CMAKE_CURRENT_BINARY_DIR}/rust-target")
set(RUST_STATIC_LIBRARY "${RUST_TARGET_DIR}/release/libaster_forge_cloud_files_macos_bridge.a")
file(
  GLOB_RECURSE RUST_BRIDGE_SOURCES
  CONFIGURE_DEPENDS
  "${MACOS_BRIDGE_ROOT}/src/*.rs"
  "${REPOSITORY_ROOT}/crates/aster_forge_cloud_files/core/src/*.rs"
)

file(MAKE_DIRECTORY "${GENERATED_ROOT}")

configure_file(
  "${CMAKE_CURRENT_SOURCE_DIR}/Shared/DomainConfiguration.swift.in"
  "${GENERATED_ROOT}/DomainConfiguration.swift"
  @ONLY
)
configure_file(
  "${CMAKE_CURRENT_SOURCE_DIR}/Host/Info.plist.in"
  "${GENERATED_ROOT}/Host-Info.plist"
  @ONLY
)
configure_file(
  "${CMAKE_CURRENT_SOURCE_DIR}/Host/Host.entitlements.in"
  "${GENERATED_ROOT}/Host.entitlements"
  @ONLY
)
configure_file(
  "${CMAKE_CURRENT_SOURCE_DIR}/Extension/Info.plist.in"
  "${GENERATED_ROOT}/Extension-Info.plist"
  @ONLY
)
configure_file(
  "${CMAKE_CURRENT_SOURCE_DIR}/Extension/Extension.entitlements.in"
  "${GENERATED_ROOT}/Extension.entitlements"
  @ONLY
)

add_custom_command(
  OUTPUT "${RUST_STATIC_LIBRARY}"
  COMMAND
    "${CMAKE_COMMAND}" -E env
    "CARGO_TARGET_DIR=${RUST_TARGET_DIR}"
    "MACOSX_DEPLOYMENT_TARGET=${CMAKE_OSX_DEPLOYMENT_TARGET}"
    cargo build --locked --release -p aster_forge_cloud_files_macos_bridge
  WORKING_DIRECTORY "${REPOSITORY_ROOT}"
  DEPENDS
    "${REPOSITORY_ROOT}/Cargo.toml"
    "${REPOSITORY_ROOT}/Cargo.lock"
    "${MACOS_BRIDGE_ROOT}/Cargo.toml"
    "${MACOS_BRIDGE_ROOT}/include/aster_forge_cloud_files_macos_bridge.h"
    ${RUST_BRIDGE_SOURCES}
  VERBATIM
)
add_custom_target(AsterForgeCloudFilesMacosRustBridge DEPENDS "${RUST_STATIC_LIBRARY}")

add_library(AsterForgeCloudFilesMacosRustArchive STATIC IMPORTED GLOBAL)
set_target_properties(
  AsterForgeCloudFilesMacosRustArchive
  PROPERTIES IMPORTED_LOCATION "${RUST_STATIC_LIBRARY}"
)
add_dependencies(AsterForgeCloudFilesMacosRustArchive AsterForgeCloudFilesMacosRustBridge)

find_library(APPKIT_FRAMEWORK AppKit REQUIRED)
find_library(FILE_PROVIDER_FRAMEWORK FileProvider REQUIRED)
find_library(SECURITY_FRAMEWORK Security REQUIRED)
find_library(SYSTEM_CONFIGURATION_FRAMEWORK SystemConfiguration REQUIRED)
find_library(UNIFORM_TYPE_IDENTIFIERS_FRAMEWORK UniformTypeIdentifiers REQUIRED)

add_executable(
  AsterForgeCloudFilesFixtureHost
  MACOSX_BUNDLE
  Host/AppDelegate.swift
  Host/SystemTestRunner.swift
  "${GENERATED_ROOT}/DomainConfiguration.swift"
  "${SWIFT_SHELL_ROOT}/Sources/AsterForgeCloudFilesMacosShell/SystemTestReport.swift"
  "${SWIFT_SHELL_ROOT}/Sources/AsterForgeCloudFilesMacosShell/WorkingSetSignaler.swift"
)
target_link_libraries(
  AsterForgeCloudFilesFixtureHost
  PRIVATE
    "${APPKIT_FRAMEWORK}"
    "${FILE_PROVIDER_FRAMEWORK}"
)
set_target_properties(
  AsterForgeCloudFilesFixtureHost
  PROPERTIES
    MACOSX_BUNDLE_INFO_PLIST "${GENERATED_ROOT}/Host-Info.plist"
    XCODE_ATTRIBUTE_CODE_SIGN_ENTITLEMENTS "${GENERATED_ROOT}/Host.entitlements"
    XCODE_ATTRIBUTE_CODE_SIGNING_ALLOWED "${ASTER_FORGE_CODE_SIGNING_ALLOWED}"
    XCODE_ATTRIBUTE_DEVELOPMENT_TEAM "${ASTER_FORGE_DEVELOPMENT_TEAM}"
    XCODE_ATTRIBUTE_ONLY_ACTIVE_ARCH YES
    XCODE_ATTRIBUTE_PRODUCT_BUNDLE_IDENTIFIER "${ASTER_FORGE_HOST_BUNDLE_ID}"
    XCODE_GENERATE_SCHEME YES
)

set(
  SWIFT_SHELL_SOURCES
  "${SWIFT_SHELL_ROOT}/Sources/AsterForgeCloudFilesMacosShell/BridgeContracts.swift"
  "${SWIFT_SHELL_ROOT}/Sources/AsterForgeCloudFilesMacosShell/FileProviderMapping.swift"
  "${SWIFT_SHELL_ROOT}/Sources/AsterForgeCloudFilesMacosShell/MaterializedItems.swift"
  "${SWIFT_SHELL_ROOT}/Sources/AsterForgeCloudFilesMacosShell/OperationState.swift"
  "${SWIFT_SHELL_ROOT}/Sources/AsterForgeCloudFilesMacosShell/ReadOnlyRuntime.swift"
  "${SWIFT_SHELL_ROOT}/Sources/AsterForgeCloudFilesMacosShell/TemporaryContentStore.swift"
  "${SWIFT_SHELL_ROOT}/Sources/AsterForgeCloudFilesMacosShell/WorkingSetSignaler.swift"
)

add_library(
  AsterForgeCloudFilesFixtureExtension
  MODULE
  Extension/FileProviderExtension.swift
  Extension/MemoryCloudDataSource.swift
  Extension/RustBridge.swift
  "${GENERATED_ROOT}/DomainConfiguration.swift"
  ${SWIFT_SHELL_SOURCES}
)
add_dependencies(AsterForgeCloudFilesFixtureExtension AsterForgeCloudFilesMacosRustBridge)
target_link_libraries(
  AsterForgeCloudFilesFixtureExtension
  PRIVATE
    AsterForgeCloudFilesMacosRustArchive
    "${FILE_PROVIDER_FRAMEWORK}"
    "${SECURITY_FRAMEWORK}"
    "${SYSTEM_CONFIGURATION_FRAMEWORK}"
    "${UNIFORM_TYPE_IDENTIFIERS_FRAMEWORK}"
)
set_target_properties(
  AsterForgeCloudFilesFixtureExtension
  PROPERTIES
    BUNDLE TRUE
    BUNDLE_EXTENSION appex
    MACOSX_BUNDLE_INFO_PLIST "${GENERATED_ROOT}/Extension-Info.plist"
    XCODE_ATTRIBUTE_CODE_SIGN_ENTITLEMENTS "${GENERATED_ROOT}/Extension.entitlements"
    XCODE_ATTRIBUTE_CODE_SIGNING_ALLOWED "${ASTER_FORGE_CODE_SIGNING_ALLOWED}"
    XCODE_ATTRIBUTE_DEVELOPMENT_TEAM "${ASTER_FORGE_DEVELOPMENT_TEAM}"
    XCODE_ATTRIBUTE_ONLY_ACTIVE_ARCH YES
    XCODE_ATTRIBUTE_PRODUCT_BUNDLE_IDENTIFIER "${ASTER_FORGE_EXTENSION_BUNDLE_ID}"
    XCODE_ATTRIBUTE_SKIP_INSTALL YES
    XCODE_ATTRIBUTE_SWIFT_OBJC_BRIDGING_HEADER
      "${CMAKE_CURRENT_SOURCE_DIR}/Extension/BridgeHeader.h"
    XCODE_EXPLICIT_FILE_TYPE wrapper.app-extension
    XCODE_GENERATE_SCHEME YES
    XCODE_PRODUCT_TYPE com.apple.product-type.app-extension
)

set_target_properties(
  AsterForgeCloudFilesFixtureHost
  PROPERTIES
    XCODE_EMBED_APP_EXTENSIONS AsterForgeCloudFilesFixtureExtension
    XCODE_EMBED_APP_EXTENSIONS_CODE_SIGN_ON_COPY YES
)
add_dependencies(AsterForgeCloudFilesFixtureHost AsterForgeCloudFilesFixtureExtension)

set(
  MEMORY_EXAMPLE_SUPPORT_SOURCE
  "${SWIFT_SHELL_ROOT}/Sources/AsterForgeCloudFilesMacosExampleSupport/MemoryCloudCommand.swift"
)

add_executable(
  AsterForgeCloudFilesMacosMemoryCloudDrive
  Examples/MemoryCloudDrive.swift
  Extension/MemoryCloudDataSource.swift
  Extension/RustBridge.swift
  "${GENERATED_ROOT}/DomainConfiguration.swift"
  "${MEMORY_EXAMPLE_SUPPORT_SOURCE}"
  ${SWIFT_SHELL_SOURCES}
)
add_dependencies(AsterForgeCloudFilesMacosMemoryCloudDrive AsterForgeCloudFilesMacosRustBridge)
target_link_libraries(
  AsterForgeCloudFilesMacosMemoryCloudDrive
  PRIVATE
    AsterForgeCloudFilesMacosRustArchive
    "${FILE_PROVIDER_FRAMEWORK}"
    "${SECURITY_FRAMEWORK}"
    "${SYSTEM_CONFIGURATION_FRAMEWORK}"
    "${UNIFORM_TYPE_IDENTIFIERS_FRAMEWORK}"
)
set_target_properties(
  AsterForgeCloudFilesMacosMemoryCloudDrive
  PROPERTIES
    OUTPUT_NAME macos_memory_cloud_drive
    XCODE_ATTRIBUTE_CODE_SIGNING_ALLOWED "${ASTER_FORGE_CODE_SIGNING_ALLOWED}"
    XCODE_ATTRIBUTE_ONLY_ACTIVE_ARCH YES
    XCODE_ATTRIBUTE_SWIFT_OBJC_BRIDGING_HEADER
      "${CMAKE_CURRENT_SOURCE_DIR}/Extension/BridgeHeader.h"
)

add_test(
  NAME AsterForgeCloudFilesMacosMemoryCloudSmoke
  COMMAND $<TARGET_FILE:AsterForgeCloudFilesMacosMemoryCloudDrive> smoke
)
set_tests_properties(
  AsterForgeCloudFilesMacosMemoryCloudSmoke
  PROPERTIES PASS_REGULAR_EXPRESSION "memory cloud example passed"
)
add_test(
  NAME AsterForgeCloudFilesMacosMemoryCloudList
  COMMAND $<TARGET_FILE:AsterForgeCloudFilesMacosMemoryCloudDrive> list root
)
set_tests_properties(
  AsterForgeCloudFilesMacosMemoryCloudList
  PROPERTIES PASS_REGULAR_EXPRESSION "README.txt.*Documents"
)
add_test(
  NAME AsterForgeCloudFilesMacosMemoryCloudNestedList
  COMMAND $<TARGET_FILE:AsterForgeCloudFilesMacosMemoryCloudDrive> list Documents
)
set_tests_properties(
  AsterForgeCloudFilesMacosMemoryCloudNestedList
  PROPERTIES PASS_REGULAR_EXPRESSION "hello.txt"
)
add_test(
  NAME AsterForgeCloudFilesMacosMemoryCloudCat
  COMMAND $<TARGET_FILE:AsterForgeCloudFilesMacosMemoryCloudDrive> cat Documents/hello.txt
)
set_tests_properties(
  AsterForgeCloudFilesMacosMemoryCloudCat
  PROPERTIES PASS_REGULAR_EXPRESSION "Hello from the in-memory cloud"
)
add_test(
  NAME AsterForgeCloudFilesMacosMemoryCloudChanges
  COMMAND $<TARGET_FILE:AsterForgeCloudFilesMacosMemoryCloudDrive> changes initial
)
set_tests_properties(
  AsterForgeCloudFilesMacosMemoryCloudChanges
  PROPERTIES PASS_REGULAR_EXPRESSION "memory-fixture-v1"
)
add_test(
  NAME AsterForgeCloudFilesMacosMemoryCloudWorkingSet
  COMMAND $<TARGET_FILE:AsterForgeCloudFilesMacosMemoryCloudDrive> working-set
)
set_tests_properties(
  AsterForgeCloudFilesMacosMemoryCloudWorkingSet
  PROPERTIES PASS_REGULAR_EXPRESSION "Documents.*hello.txt"
)
add_test(
  NAME AsterForgeCloudFilesMacosMemoryCloudRejectsTraversal
  COMMAND
    "${CMAKE_COMMAND}"
    "-DCOMMAND_PATH=$<TARGET_FILE:AsterForgeCloudFilesMacosMemoryCloudDrive>"
    "-DCOMMAND_ARGUMENTS=cat|../README.txt"
    "-DEXPECTED_REGEX=invalid memory-cloud path"
    -P "${CMAKE_CURRENT_SOURCE_DIR}/cmake/AssertCommandFailure.cmake"
)
add_test(
  NAME AsterForgeCloudFilesMacosMemoryCloudRejectsMissingItem
  COMMAND
    "${CMAKE_COMMAND}"
    "-DCOMMAND_PATH=$<TARGET_FILE:AsterForgeCloudFilesMacosMemoryCloudDrive>"
    "-DCOMMAND_ARGUMENTS=cat|missing.txt"
    "-DEXPECTED_REGEX=missing.txt was not found"
    -P "${CMAKE_CURRENT_SOURCE_DIR}/cmake/AssertCommandFailure.cmake"
)
add_test(
  NAME AsterForgeCloudFilesMacosMemoryCloudRejectsListFile
  COMMAND
    "${CMAKE_COMMAND}"
    "-DCOMMAND_PATH=$<TARGET_FILE:AsterForgeCloudFilesMacosMemoryCloudDrive>"
    "-DCOMMAND_ARGUMENTS=list|README.txt"
    "-DEXPECTED_REGEX=README.txt is not a directory"
    -P "${CMAKE_CURRENT_SOURCE_DIR}/cmake/AssertCommandFailure.cmake"
)
add_test(
  NAME AsterForgeCloudFilesMacosMemoryCloudRejectsCatDirectory
  COMMAND
    "${CMAKE_COMMAND}"
    "-DCOMMAND_PATH=$<TARGET_FILE:AsterForgeCloudFilesMacosMemoryCloudDrive>"
    "-DCOMMAND_ARGUMENTS=cat|Documents"
    "-DEXPECTED_REGEX=Documents is not a file"
    -P "${CMAKE_CURRENT_SOURCE_DIR}/cmake/AssertCommandFailure.cmake"
)
add_test(
  NAME AsterForgeCloudFilesMacosMemoryCloudRejectsExpiredAnchor
  COMMAND
    "${CMAKE_COMMAND}"
    "-DCOMMAND_PATH=$<TARGET_FILE:AsterForgeCloudFilesMacosMemoryCloudDrive>"
    "-DCOMMAND_ARGUMENTS=changes|expired"
    "-DEXPECTED_REGEX=NSFileProviderErrorDomain Code=-1002"
    -P "${CMAKE_CURRENT_SOURCE_DIR}/cmake/AssertCommandFailure.cmake"
)
