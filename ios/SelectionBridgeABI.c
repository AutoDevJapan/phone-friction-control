/* Compile this against SelectionBridge.h, then against the actual Swift-emitted
 * declarations in the pinned Apple compiler job. This is not Swift ABI proof. */
#include "SelectionBridge.h"
#include "SelectionSession.h"

_Static_assert(sizeof(int32_t) == 4, "status width");
_Static_assert(sizeof(intptr_t) == sizeof(void *), "Swift Int width");

static void *(*const decode_signature)(const uint8_t *, intptr_t) = pfc_selection_decode;
static int32_t (*const encode_signature)(void *, void *, pfc_selection_sink) = pfc_selection_encode;
static int32_t (*const equal_signature)(void *, void *) = pfc_selection_equal;
static void (*const release_signature)(void *) = pfc_selection_release;
static void *(*const session_open_signature)(const uint8_t *, intptr_t, const pfc_selection_api *) = pfc_selection_session_open;
static int32_t (*const session_restore_signature)(const void *, void *, pfc_selection_sink) = pfc_selection_session_restored;
static int32_t (*const session_commit_signature)(void *, void *) = pfc_selection_session_commit;
static void (*const session_release_signature)(void *) = pfc_selection_session_release;
_Static_assert(sizeof(pfc_selection_api) == 4 * sizeof(void (*)(void)), "provider table slots");
_Static_assert(_Alignof(pfc_selection_api) == _Alignof(void (*)(void)), "provider table alignment");
_Static_assert(offsetof(pfc_selection_api, decode) == 0, "decode slot");
_Static_assert(offsetof(pfc_selection_api, encode) == sizeof(void (*)(void)), "encode slot");
_Static_assert(offsetof(pfc_selection_api, equal) == 2 * sizeof(void (*)(void)), "equal slot");
_Static_assert(offsetof(pfc_selection_api, release) == 3 * sizeof(void (*)(void)), "release slot");
_Static_assert(_Generic(((pfc_selection_api *)0)->decode,
    void *(*)(const uint8_t *, intptr_t): 1, default: 0), "decode type");
_Static_assert(_Generic(((pfc_selection_api *)0)->encode,
    int32_t (*)(void *, void *, pfc_selection_sink): 1, default: 0), "encode type");
_Static_assert(_Generic(((pfc_selection_api *)0)->equal,
    int32_t (*)(void *, void *): 1, default: 0), "equal type");
_Static_assert(_Generic(((pfc_selection_api *)0)->release,
    void (*)(void *): 1, default: 0), "release type");

void pfc_check_selection_declarations(void) {
    (void)decode_signature;
    (void)encode_signature;
    (void)equal_signature;
    (void)release_signature;
    (void)session_open_signature;
    (void)session_restore_signature;
    (void)session_commit_signature;
    (void)session_release_signature;
}
