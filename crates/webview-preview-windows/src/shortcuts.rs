//! Accelerator reservation for trusted application webviews.
use webview2_com::{
    AcceleratorKeyPressedEventHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_PHYSICAL_KEY_STATUS,
        ICoreWebView2Controller,
    },
};
use windows::{
    Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    },
    core::Result,
};

/// Reserves unmodified F12 for the trusted app's debugger toggle.
///
/// Call once from the controller's UI thread (for example, Tauri's `with_webview`).
/// The controller owns the callback until its webview closes. Captured-page
/// webviews must not install this application shortcut.
/// # Errors
/// Returns a `WebView2` error if accelerator registration fails.
pub fn reserve_debugger_toggle(
    controller: &ICoreWebView2Controller,
    toggle: impl Fn() + Send + Sync + 'static,
) -> Result<()> {
    let mut registration = 0;
    // SAFETY: The caller supplies its live UI-thread controller. Event arguments
    // are owned COM references valid for this callback; output pointers refer to
    // initialized stack values. GetKeyState reads this thread's keyboard state.
    unsafe {
        controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let mut key = 0;
                args.VirtualKey(&raw mut key)?;
                if key != 0x7b
                    || [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
                        .iter()
                        .any(|key| GetKeyState(i32::from(key.0)) < 0)
                {
                    return Ok(());
                }
                args.SetHandled(true)?;
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN;
                args.KeyEventKind(&raw mut kind)?;
                let mut status = COREWEBVIEW2_PHYSICAL_KEY_STATUS::default();
                args.PhysicalKeyStatus(&raw mut status)?;
                if kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN && !status.WasKeyDown.as_bool() {
                    toggle();
                }
                Ok(())
            })),
            &raw mut registration,
        )
    }
}
