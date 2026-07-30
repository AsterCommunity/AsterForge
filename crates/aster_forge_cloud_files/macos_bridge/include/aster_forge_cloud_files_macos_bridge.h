#ifndef ASTER_FORGE_CLOUD_FILES_MACOS_BRIDGE_H
#define ASTER_FORGE_CLOUD_FILES_MACOS_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef enum AsterForgeMacosErrorCode {
  ASTER_FORGE_MACOS_SUCCESS = 0,
  ASTER_FORGE_MACOS_NOT_FOUND = 1,
  ASTER_FORGE_MACOS_NOT_AUTHENTICATED = 2,
  ASTER_FORGE_MACOS_PERMISSION_DENIED = 3,
  ASTER_FORGE_MACOS_VERSION_OUT_OF_DATE = 4,
  ASTER_FORGE_MACOS_TRY_AGAIN = 5,
  ASTER_FORGE_MACOS_NOT_SUPPORTED = 6,
  ASTER_FORGE_MACOS_INVALID_ARGUMENT = 7,
  ASTER_FORGE_MACOS_SYNC_ANCHOR_EXPIRED = 8,
  ASTER_FORGE_MACOS_CANCELLED = 9,
  ASTER_FORGE_MACOS_PROVIDER_NOT_FOUND = 10,
  ASTER_FORGE_MACOS_INTERNAL = 11,
} AsterForgeMacosErrorCode;

typedef struct AsterForgeMacosBuffer {
  uint8_t *ptr;
  size_t len;
  size_t capacity;
} AsterForgeMacosBuffer;

typedef struct AsterForgeMacosResult {
  AsterForgeMacosErrorCode code;
  AsterForgeMacosBuffer buffer;
} AsterForgeMacosResult;

typedef struct AsterForgeMacosSessionHandle {
  const void *raw;
} AsterForgeMacosSessionHandle;

typedef struct AsterForgeMacosRequestHandle {
  void *raw;
} AsterForgeMacosRequestHandle;

typedef struct AsterForgeMacosSessionResult {
  AsterForgeMacosErrorCode code;
  AsterForgeMacosSessionHandle handle;
} AsterForgeMacosSessionResult;

typedef struct AsterForgeMacosRequestResult {
  AsterForgeMacosErrorCode code;
  AsterForgeMacosRequestHandle handle;
} AsterForgeMacosRequestResult;

/* Every non-null input pointer must remain readable for its declared length until return. */
AsterForgeMacosResult aster_forge_cloud_files_macos_identifier_encode(
    const uint8_t *namespace_ptr, size_t namespace_len,
    const uint8_t *root_ptr, size_t root_len,
    const uint8_t *item_ptr, size_t item_len);

AsterForgeMacosResult aster_forge_cloud_files_macos_identifier_decode(
    const uint8_t *identifier_ptr, size_t identifier_len);

/* Releases one successful result buffer exactly once; the canonical empty buffer is a no-op. */
void aster_forge_cloud_files_macos_buffer_release(AsterForgeMacosBuffer buffer);

/* Generation values are non-zero and identify one exact extension instance. */
AsterForgeMacosSessionResult aster_forge_cloud_files_macos_session_create(
    uint64_t generation);

AsterForgeMacosRequestResult aster_forge_cloud_files_macos_session_begin_request(
    AsterForgeMacosSessionHandle session, uint64_t generation);

AsterForgeMacosErrorCode aster_forge_cloud_files_macos_session_begin_closing(
    AsterForgeMacosSessionHandle session);

AsterForgeMacosErrorCode aster_forge_cloud_files_macos_session_mark_disconnected(
    AsterForgeMacosSessionHandle session);

/* Each non-null opaque handle must be live, unmodified, and released exactly once. */
void aster_forge_cloud_files_macos_request_release(
    AsterForgeMacosRequestHandle request);

void aster_forge_cloud_files_macos_session_release(
    AsterForgeMacosSessionHandle session);

#ifdef __cplusplus
}
#endif

#endif
