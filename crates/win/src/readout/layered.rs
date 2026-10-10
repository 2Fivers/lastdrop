//! The readout's pixels on screen: a top-down 32-bit DIB section in a memory DC, filled from the
//! canvas (premultiplied BGRA) and pushed with `UpdateLayeredWindow(ULW_ALPHA)` (R§6.1).
//!
//! One surface per size; the GDI objects are released on drop, so a resize or a DPI change
//! never leaks handles (R§12 "no GDI/USER handle growth").

use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC,
    HGDIOBJ, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};

/// A DIB section selected into its own memory DC.
pub(super) struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
    width: u32,
    height: u32,
}

impl Surface {
    /// A `width × height` surface.
    pub(super) fn new(width: u32, height: u32) -> windows::core::Result<Surface> {
        let (w, h) = (
            i32::try_from(width.max(1)).unwrap_or(1),
            i32::try_from(height.max(1)).unwrap_or(1),
        );
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: u32::try_from(size_of::<BITMAPINFOHEADER>()).unwrap_or(40),
                biWidth: w,
                // Negative: top-down rows, as the canvas delivers them.
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        // SAFETY: a memory DC compatible with the screen.
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return Err(windows::core::Error::from_thread());
        }
        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        // SAFETY: `info` describes a 32-bit top-down DIB; `bits` receives its memory.
        let bitmap = match unsafe {
            CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        } {
            Ok(b) if !bits.is_null() => b,
            Ok(b) => {
                // SAFETY: both handles were created above and are not selected anywhere.
                unsafe {
                    let _ = DeleteObject(b.into());
                    let _ = DeleteDC(dc);
                }
                return Err(windows::Win32::Foundation::E_OUTOFMEMORY.into());
            }
            Err(e) => {
                // SAFETY: the DC was created above.
                unsafe {
                    let _ = DeleteDC(dc);
                }
                return Err(e);
            }
        };
        // SAFETY: selects our bitmap into our DC; the previous object is restored on drop.
        let previous = unsafe { SelectObject(dc, bitmap.into()) };
        Ok(Surface {
            dc,
            bitmap,
            previous,
            bits: bits.cast(),
            width: width.max(1),
            height: height.max(1),
        })
    }

    /// Width and height in device px.
    pub(super) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Copies premultiplied BGRA rows (stride `width × 4`) into the DIB.
    pub(super) fn write(&mut self, pixels: &[u8]) {
        let len = (self.width as usize) * (self.height as usize) * 4;
        let n = len.min(pixels.len());
        // SAFETY: the DIB section holds `width × height × 4` bytes (32 bpp, no row padding);
        // `n` never exceeds it or the source.
        unsafe { core::ptr::copy_nonoverlapping(pixels.as_ptr(), self.bits, n) };
    }

    /// Pushes the DIB to `hwnd` at `pos` (screen device px), sizing the window to the surface.
    pub(super) fn present(&self, hwnd: HWND, pos: (i32, i32)) -> windows::core::Result<()> {
        let dst = POINT { x: pos.0, y: pos.1 };
        let size = SIZE {
            cx: i32::try_from(self.width).unwrap_or(1),
            cy: i32::try_from(self.height).unwrap_or(1),
        };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: u8::try_from(AC_SRC_OVER).unwrap_or(0),
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: u8::try_from(AC_SRC_ALPHA).unwrap_or(1),
        };
        // SAFETY: every pointer is a local valid for the call; the DC holds our DIB.
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                None,
                Some(&dst),
                Some(&size),
                Some(self.dc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: restores the DC's original bitmap, then frees what `new` created.
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}
