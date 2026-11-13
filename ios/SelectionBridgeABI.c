/* Compile this against SelectionBridge.h, then against the actual Swift-emitted
 * declarations in the pinned Apple compiler job. This is not Swift ABI proof. */
#include "SelectionBridge.h"

_Static_assert(sizeof(int32_t) == 4, "status width");
_Static_assert(sizeof(intptr_t) == sizeof(void *), "Swift Int width");

static void *(*const decode_signature)(const uint8_t *, intptr_t) = pfc_selection_decode;
static int32_t (*const encode_signature)(void *, void *, pfc_selection_sink) = pfc_selection_encode;
static int32_t (*const equal_signature)(void *, void *) = pfc_selection_equal;
static void (*const release_signature)(void *) = pfc_selection_release;

void pfc_check_selection_declarations(void) {
    (void)decode_signature;
    (void)encode_signature;
    (void)equal_signature;
    (void)release_signature;
}
