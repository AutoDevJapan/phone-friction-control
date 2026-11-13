#ifndef PFC_SELECTION_SESSION_H
#define PFC_SELECTION_SESSION_H
#include "SelectionBridge.h"
typedef struct {
    void *(*decode)(const uint8_t *, intptr_t);
    int32_t (*encode)(void *, void *, pfc_selection_sink);
    int32_t (*equal)(void *, void *);
    void (*release)(void *);
} pfc_selection_api;
/* All operations are serialized on main. The provider must outlive the session.
 * open copies UTF-8 path and the function table; null indicates failure.
 * restored: 1 = absent (no sink), 0 = one synchronous sink accepted, <0 = error.
 * commit consumes Selection +1 on every result for a valid session.
 * Null/stale session is a caller contract violation; null returns -1 without
 * consuming selection. release consumes the session once, no outstanding calls.
 */
void *pfc_selection_session_open(const uint8_t *, intptr_t, const pfc_selection_api *);
int32_t pfc_selection_session_restored(const void *, void *, pfc_selection_sink);
int32_t pfc_selection_session_commit(void *, void *);
void pfc_selection_session_release(void *);
#endif
