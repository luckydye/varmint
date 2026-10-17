use objc2::{AnyThread, runtime::AnyObject};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSBitmapImageRepPropertyKey, NSDeviceRGBColorSpace};
use objc2_foundation::NSDictionary;
use objc2_io_surface::{IOSurfaceLockOptions, IOSurfaceRef};
use std::ptr;
use zerocopy::IntoBytes;

/// A copy of the current scanout, taken under the display lock.
pub enum Capture {
    Pixels { pixels: Vec<u32>, width: usize, height: usize },
    IOSurface { id: u32, width: usize, height: usize },
}

impl Capture {
    pub fn size(&self) -> (usize, usize) {
        match *self {
            Self::Pixels { width, height, .. } | Self::IOSurface { width, height, .. } => (width, height),
        }
    }

    /// The pixels as tightly packed RGB rows.
    pub fn rgb(&self) -> Result<Vec<u8>, String> {
        let (width, height) = self.size();
        match self {
            Self::Pixels { pixels, .. } => Ok(bgra_to_rgb(pixels.as_bytes(), width * 4, width, height)),
            Self::IOSurface { id, .. } => read_iosurface(*id, width, height),
        }
    }

    pub fn encode_png(&self) -> Result<Vec<u8>, String> {
        let (width, height) = self.size();
        encode_png(&self.rgb()?, width, height)
    }
}

fn read_iosurface(id: u32, width: usize, height: usize) -> Result<Vec<u8>, String> {
    let surface = IOSurfaceRef::lookup(id).ok_or("scanout IOSurface was released before it could be read")?;
    if surface.width() < width || surface.height() < height {
        return Err("scanout IOSurface is smaller than the display".into());
    }

    let status = unsafe { surface.lock(IOSurfaceLockOptions::ReadOnly, ptr::null_mut()) };
    if status != 0 {
        return Err(format!("IOSurfaceLock failed: {status}"));
    }
    let stride = surface.bytes_per_row();
    let bytes = unsafe { std::slice::from_raw_parts(surface.base_address().as_ptr().cast::<u8>(), stride * height) };
    let rgb = bgra_to_rgb(bytes, stride, width, height);
    unsafe { surface.unlock(IOSurfaceLockOptions::ReadOnly, ptr::null_mut()) };

    Ok(rgb)
}

fn bgra_to_rgb(bytes: &[u8], stride: usize, width: usize, height: usize) -> Vec<u8> {
    let mut rgb = vec![0; width * height * 3];
    for (row, out) in bytes.chunks(stride).zip(rgb.chunks_exact_mut(width * 3)) {
        for (pixel, out) in row[..width * 4].chunks_exact(4).zip(out.chunks_exact_mut(3)) {
            out.copy_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
    }
    rgb
}

fn encode_png(rgb: &[u8], width: usize, height: usize) -> Result<Vec<u8>, String> {
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            ptr::null_mut(),
            width as isize,
            height as isize,
            8,
            3,
            false,
            false,
            NSDeviceRGBColorSpace,
            (width * 3) as isize,
            24,
        )
    }
    .ok_or("failed to allocate a bitmap for the screenshot")?;

    unsafe { std::slice::from_raw_parts_mut(rep.bitmapData(), rgb.len()) }.copy_from_slice(rgb);

    let properties = NSDictionary::<NSBitmapImageRepPropertyKey, AnyObject>::new();
    let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties) }
        .ok_or("failed to encode the screenshot as PNG")?;
    Ok(data.to_vec())
}
