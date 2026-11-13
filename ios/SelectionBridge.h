#ifndef PFC_SELECTION_BRIDGE_H
#define PFC_SELECTION_BRIDGE_H
#include <stdint.h>
#include <stddef.h>

/* Candidate ABI, not linked by the current Cargo entry.
 * Handles originate only from this bridge; release each owned handle once.
 * All calls require the main thread and a live handle with no concurrent use.
 * Sink bytes are borrowed only until the synchronous callback returns.
 * Sink must copy the bytes, must not unwind or reenter, and returns 0 on success.
 */
typedef int32_t (*pfc_selection_sink)(void *, const uint8_t *, intptr_t);
void *pfc_selection_decode(const uint8_t *bytes, intptr_t count);
int32_t pfc_selection_encode(void *handle, void *context, pfc_selection_sink sink);
int32_t pfc_selection_equal(void *left, void *right);
void pfc_selection_release(void *handle);
#endif
