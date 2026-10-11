//! The readout's UI Automation provider (R§11): the window answers `WM_GETOBJECT` with a root
//! element — a `Button` named `Lastdrop. ` + the row sentences, HelpText, LiveSetting Polite,
//! Invoke = open the flyout — and one `ProgressBar` child per drawn row with its sentence and a
//! read-only RangeValue (`pct`, 0–100).
//!
//! Threading: the provider is registered with `ProviderOptions_UseComThreading`, so UI
//! Automation calls it on the readout thread (an STA with a message loop). Its data is a copy the
//! readout thread swaps in after each frame ([`UiaData`]); the lock is held only to copy values,
//! never across a Win32 or COM call.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use windows::Win32::Foundation::{E_ACCESSDENIED, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Accessibility::{
    IInvokeProvider, IInvokeProvider_Impl, IRangeValueProvider, IRangeValueProvider_Impl,
    IRawElementProviderFragment, IRawElementProviderFragment_Impl, IRawElementProviderFragmentRoot,
    IRawElementProviderFragmentRoot_Impl, IRawElementProviderSimple,
    IRawElementProviderSimple_Impl, NavigateDirection, NavigateDirection_FirstChild,
    NavigateDirection_LastChild, NavigateDirection_NextSibling, NavigateDirection_Parent,
    NavigateDirection_PreviousSibling, Polite, ProviderOptions, ProviderOptions_ServerSideProvider,
    ProviderOptions_UseComThreading, UIA_AutomationIdPropertyId, UIA_ButtonControlTypeId,
    UIA_ControlTypePropertyId, UIA_HelpTextPropertyId, UIA_InvokePatternId,
    UIA_IsContentElementPropertyId, UIA_IsControlElementPropertyId,
    UIA_IsKeyboardFocusablePropertyId, UIA_LiveRegionChangedEventId, UIA_LiveSettingPropertyId,
    UIA_NamePropertyId, UIA_PATTERN_ID, UIA_PROPERTY_ID, UIA_ProgressBarControlTypeId,
    UIA_RangeValuePatternId, UiaAppendRuntimeId, UiaDisconnectProvider, UiaHostProviderFromHwnd,
    UiaRaiseAutomationEvent, UiaRect, UiaReturnRawElementProvider, UiaRootObjectId,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::{BOOL, Error, IUnknown, IUnknownImpl, Interface, Result, implement};

use crate::geom::Rect;

/// One row element.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct UiaRow {
    /// The row sentence (R§11).
    pub name: String,
    /// RangeValue `pct`; `None` while LOADING.
    pub value: Option<u8>,
    /// Screen rectangle, device px.
    pub rect: Rect,
}

/// What the provider reports, swapped in by the readout thread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct UiaData {
    /// Root name: `Lastdrop. ` + the row sentences (compact form: the compact row's).
    pub name: String,
    /// HelpText.
    pub help: String,
    /// The rows drawn.
    pub rows: Vec<UiaRow>,
    /// The slot on screen; empty while hidden.
    pub slot: Rect,
}

/// Shared provider data.
#[derive(Default)]
pub(super) struct Shared {
    data: Mutex<UiaData>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, UiaData> {
        // Plain values: a panic elsewhere cannot leave them inconsistent.
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the data (readout thread).
    pub(super) fn set(&self, data: UiaData) {
        *self.lock() = data;
    }

    fn snapshot(&self) -> UiaData {
        self.lock().clone()
    }
}

/// The root provider with what the readout thread needs to keep for it.
pub(super) struct Provider {
    root: IRawElementProviderSimple,
    shared: Arc<Shared>,
}

impl Provider {
    /// A provider for the readout window `hwnd`; Invoke posts `invoke_msg` to `notify`.
    pub(super) fn new(hwnd: HWND, notify: HWND, invoke_msg: u32) -> Provider {
        let shared = Arc::new(Shared::default());
        let root: IRawElementProviderSimple = Root {
            hwnd: hwnd.0 as isize,
            notify: notify.0 as isize,
            invoke_msg,
            shared: Arc::clone(&shared),
        }
        .into();
        Provider { root, shared }
    }

    /// The data the provider reports.
    pub(super) fn shared(&self) -> &Shared {
        &self.shared
    }

    /// Answers `WM_GETOBJECT` for the UIA root (`lParam == UiaRootObjectId`); `None` for other
    /// object ids (the caller uses `DefWindowProc`).
    pub(super) fn answer(&self, hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> Option<isize> {
        // The object id is a 32-bit value in the low half of lParam.
        if lparam.0 as u32 as i32 != UiaRootObjectId {
            return None;
        }
        // SAFETY: hands UIA our provider for this window and message.
        Some(unsafe { UiaReturnRawElementProvider(hwnd, wparam, lparam, &self.root) }.0)
    }

    /// Raises `UIA_LiveRegionChangedEventId` on the root (R§11: severity rose, stale/fresh,
    /// reset).
    pub(super) fn announce(&self) {
        // SAFETY: a live provider; failures (nobody listening) are harmless.
        if let Err(e) = unsafe { UiaRaiseAutomationEvent(&self.root, UIA_LiveRegionChangedEventId) }
        {
            log::debug!("live region event not raised: {e}");
        }
    }

    /// Releases UIA's references before the window goes away.
    pub(super) fn disconnect(&self, hwnd: HWND) {
        // SAFETY: the documented clean-up: no more providers for this window, then disconnect.
        unsafe {
            UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
            let _ = UiaDisconnectProvider(&self.root);
        }
    }
}

fn hwnd_of(raw: isize) -> HWND {
    HWND(raw as *mut core::ffi::c_void)
}

fn uia_rect(r: Rect) -> UiaRect {
    UiaRect {
        left: f64::from(r.left),
        top: f64::from(r.top),
        width: f64::from(r.width().max(0)),
        height: f64::from(r.height().max(0)),
    }
}

/// A null interface pointer with `S_OK` (UIA's "no element").
fn none<T>() -> Result<T> {
    Err(Error::empty())
}

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot,
    IInvokeProvider
)]
struct Root {
    hwnd: isize,
    notify: isize,
    invoke_msg: u32,
    shared: Arc<Shared>,
}

impl Root_Impl {
    fn row(&self, index: usize) -> Result<IRawElementProviderFragment> {
        if index >= self.shared.lock().rows.len() {
            return none();
        }
        Ok(Row {
            index,
            root: self.to_interface(),
            shared: Arc::clone(&self.shared),
        }
        .into())
    }
}

impl IRawElementProviderSimple_Impl for Root_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider | ProviderOptions_UseComThreading)
    }

    fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
        if pattern == UIA_InvokePatternId {
            let invoke: IInvokeProvider = self.to_interface();
            return Ok(invoke.into());
        }
        none()
    }

    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let data = self.shared.snapshot();
        let p = property;
        Ok(if p == UIA_NamePropertyId {
            VARIANT::from(data.name.as_str())
        } else if p == UIA_HelpTextPropertyId {
            VARIANT::from(data.help.as_str())
        } else if p == UIA_ControlTypePropertyId {
            VARIANT::from(UIA_ButtonControlTypeId.0)
        } else if p == UIA_LiveSettingPropertyId {
            VARIANT::from(Polite.0)
        } else if p == UIA_AutomationIdPropertyId {
            VARIANT::from("LastdropReadout")
        } else if p == UIA_IsKeyboardFocusablePropertyId {
            VARIANT::from(false)
        } else if p == UIA_IsControlElementPropertyId || p == UIA_IsContentElementPropertyId {
            VARIANT::from(true)
        } else {
            VARIANT::default()
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        // SAFETY: our own window handle.
        unsafe { UiaHostProviderFromHwnd(hwnd_of(self.hwnd)) }
    }
}

impl IRawElementProviderFragment_Impl for Root_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let count = self.shared.lock().rows.len();
        if count == 0 {
            none()
        } else if direction == NavigateDirection_FirstChild {
            self.row(0)
        } else if direction == NavigateDirection_LastChild {
            self.row(count - 1)
        } else {
            none()
        }
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        // A root hosted by a window has no runtime id of its own.
        Ok(core::ptr::null_mut())
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        Ok(uia_rect(self.shared.lock().slot))
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(core::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        // Not focusable (WS_EX_NOACTIVATE, R§10).
        Ok(())
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(self.to_interface())
    }
}

impl IRawElementProviderFragmentRoot_Impl for Root_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let hit = {
            let data = self.shared.lock();
            data.rows.iter().position(|r| {
                x >= f64::from(r.rect.left)
                    && x < f64::from(r.rect.right)
                    && y >= f64::from(r.rect.top)
                    && y < f64::from(r.rect.bottom)
            })
        };
        match hit {
            Some(i) => self.row(i),
            None => Ok(self.to_interface()),
        }
    }

    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        none()
    }
}

impl IInvokeProvider_Impl for Root_Impl {
    fn Invoke(&self) -> Result<()> {
        // SAFETY: posting to the readout thread's own message window.
        unsafe {
            PostMessageW(
                Some(hwnd_of(self.notify)),
                self.invoke_msg,
                WPARAM(0),
                LPARAM(0),
            )
        }
    }
}

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRangeValueProvider
)]
struct Row {
    index: usize,
    root: IRawElementProviderFragmentRoot,
    shared: Arc<Shared>,
}

impl Row_Impl {
    fn data(&self) -> Option<UiaRow> {
        self.shared.lock().rows.get(self.index).cloned()
    }

    fn sibling(&self, index: usize) -> Result<IRawElementProviderFragment> {
        if index >= self.shared.lock().rows.len() {
            return none();
        }
        Ok(Row {
            index,
            root: self.root.clone(),
            shared: Arc::clone(&self.shared),
        }
        .into())
    }
}

impl IRawElementProviderSimple_Impl for Row_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider | ProviderOptions_UseComThreading)
    }

    fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
        if pattern == UIA_RangeValuePatternId {
            let range: IRangeValueProvider = self.to_interface();
            return Ok(range.into());
        }
        none()
    }

    fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let row = self.data().unwrap_or_default();
        let p = property;
        Ok(if p == UIA_NamePropertyId {
            VARIANT::from(row.name.as_str())
        } else if p == UIA_ControlTypePropertyId {
            VARIANT::from(UIA_ProgressBarControlTypeId.0)
        } else if p == UIA_AutomationIdPropertyId {
            VARIANT::from(format!("LastdropReadoutRow{}", self.index + 1).as_str())
        } else if p == UIA_IsKeyboardFocusablePropertyId {
            VARIANT::from(false)
        } else if p == UIA_IsControlElementPropertyId || p == UIA_IsContentElementPropertyId {
            VARIANT::from(true)
        } else {
            VARIANT::default()
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        none()
    }
}

impl IRawElementProviderFragment_Impl for Row_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        if direction == NavigateDirection_Parent {
            self.root.cast()
        } else if direction == NavigateDirection_NextSibling {
            self.sibling(self.index + 1)
        } else if direction == NavigateDirection_PreviousSibling && self.index > 0 {
            self.sibling(self.index - 1)
        } else {
            none()
        }
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        let id = [
            i32::try_from(UiaAppendRuntimeId).unwrap_or(3),
            i32::try_from(self.index + 1).unwrap_or(i32::MAX),
        ];
        // SAFETY: a two-element VT_I4 vector; each element is copied from a local. The array is
        // ours until it is returned (UI Automation then owns it); when an element cannot be put,
        // it is destroyed here once and not returned.
        unsafe {
            let array = SafeArrayCreateVector(VT_I4, 0, 2);
            if array.is_null() {
                return Err(windows::Win32::Foundation::E_OUTOFMEMORY.into());
            }
            for (i, v) in id.iter().enumerate() {
                let index = i32::try_from(i).unwrap_or(0);
                if let Err(e) = SafeArrayPutElement(array, &index, core::ptr::from_ref(v).cast()) {
                    let _ = SafeArrayDestroy(array);
                    return Err(e);
                }
            }
            Ok(array)
        }
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        Ok(uia_rect(self.data().map(|r| r.rect).unwrap_or_default()))
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(core::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        Ok(())
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(self.root.clone())
    }
}

impl IRangeValueProvider_Impl for Row_Impl {
    fn SetValue(&self, _value: f64) -> Result<()> {
        Err(E_ACCESSDENIED.into())
    }

    fn Value(&self) -> Result<f64> {
        Ok(self.data().and_then(|r| r.value).map_or(0.0, f64::from))
    }

    fn IsReadOnly(&self) -> Result<BOOL> {
        Ok(true.into())
    }

    fn Maximum(&self) -> Result<f64> {
        Ok(100.0)
    }

    fn Minimum(&self) -> Result<f64> {
        Ok(0.0)
    }

    fn LargeChange(&self) -> Result<f64> {
        Ok(0.0)
    }

    fn SmallChange(&self) -> Result<f64> {
        Ok(0.0)
    }
}
