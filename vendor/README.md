# Android lifecycle patches

Sources: crates.io winit 0.30.13 and eframe 0.36.2, copied without changing registry sources. Original copyright and licenses are retained.

winit: Android Destroy exits the event loop. After run returns, drop the old loop before allowing another Android event loop. Other platforms retain their creation restrictions.

winit's NativeActivity input path leaves the system Back key unhandled so Android can invoke the application's Java Back handler. That handler moves the task to the background. Other keys and GameActivity retain their existing routing.

eframe: save_and_destroy accepts a missing root window, matching the existing save-on-suspend behavior. This avoids a panic after Android has removed its surface.

The application's Android entry uses run_and_return=false so the old loop is not retained in eframe's thread-local cache. Destroy causes LoopExiting to save and destroy the UI, then returns from android_main; it does not explicitly kill the process.
Lifecycle invariant: Destroy is processed by the old Android event thread; LoopExiting saves application state and releases UI resources even after its root window is gone. The old loop is dropped before Android's creation guard is reset. android_main then returns, allowing android-activity to release NativeActivity.onDestroy's wait. New activity creation remains serialized by Android's main thread. No non-Android event-loop creation policy changes.

Runtime evidence is maintained separately; successful configuration recreation does not certify every update, OS, or active-break case.
