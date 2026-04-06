/// Build script — generates a monitor-shaped tray icon as icon.ico
/// and writes it next to the compiled binary so the app can load it.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());

    let ico = build_ico();

    // Write next to the compiled binary (where the app looks at runtime).
    // OUT_DIR is typically `target/{profile}/build/monitor-ctrl-hash/out`.
    // Moving up 3 directories places us at `target/{profile}`.
    let target_dir = out_dir.parent().unwrap().parent().unwrap().parent().unwrap();
    let runtime_assets = target_dir.join("assets");
    std::fs::create_dir_all(&runtime_assets).ok();
    std::fs::write(runtime_assets.join("icon.ico"), &ico)
        .expect("failed to write target assets/icon.ico");

    // Also keep a copy in the project assets folder (for packaging / reference)
    let project_assets = format!("{}/assets", manifest_dir);
    std::fs::create_dir_all(&project_assets).ok();
    std::fs::write(format!("{}/icon.ico", project_assets), &ico)
        .expect("failed to write assets/icon.ico");
}

// ── Pixel art ──────────────────────────────────────────────────────────────

/// RGBA colour constants
const T:  [u8; 4] = [0,   0,   0,   0  ]; // transparent
const FR: [u8; 4] = [58,  58,  58,  255]; // monitor frame / bezel  (#3A3A3A)
const SC: [u8; 4] = [12,  22,  38,  255]; // screen background (dark navy)
const GL: [u8; 4] = [26,  107, 154, 255]; // screen glow (medium blue)
const HL: [u8; 4] = [91,  164, 207, 255]; // screen highlight (light blue)
const ST: [u8; 4] = [78,  78,  78,  255]; // stand / base  (#4E4E4E)

fn render_32x32() -> Vec<u8> {
    let mut buf = vec![T; 32 * 32];

    // Monitor outer bezel — x:[1,30], y:[1,21]
    fill(&mut buf, 1, 1, 30, 21, FR);
    // Screen surface — x:[3,28], y:[3,19]
    fill(&mut buf, 3, 3, 28, 19, SC);
    // Glow layer — x:[4,27], y:[4,18]
    fill(&mut buf, 4, 4, 27, 18, GL);
    // Highlight band across top of screen — x:[4,27], y:[4,8]
    fill(&mut buf, 4, 4, 27, 8, HL);
    // Dark content area (like a window on the screen) — x:[6,22], y:[10,17]
    fill(&mut buf, 6, 10, 22, 17, SC);

    // Stand neck — x:[14,17], y:[22,25]
    fill(&mut buf, 14, 22, 17, 25, ST);
    // Stand base — x:[8,23], y:[26,27]
    fill(&mut buf, 8, 26, 23, 27, ST);

    // Flatten to u8
    buf.iter().flat_map(|p| *p).collect()
}

fn render_16x16() -> Vec<u8> {
    let mut buf = vec![T; 16 * 16];

    // Bezel — x:[0,15], y:[0,10]
    fill16(&mut buf, 0, 0, 15, 10, FR);
    // Screen — x:[1,14], y:[1,9]
    fill16(&mut buf, 1, 1, 14, 9, SC);
    // Glow — x:[2,13], y:[2,8]
    fill16(&mut buf, 2, 2, 13, 8, GL);
    // Highlight — x:[2,13], y:[2,4]
    fill16(&mut buf, 2, 2, 13, 4, HL);
    // Dark content — x:[3,10], y:[5,8]
    fill16(&mut buf, 3, 5, 10, 8, SC);

    // Stand neck — x:[6,9], y:[11,12]
    fill16(&mut buf, 6, 11, 9, 12, ST);
    // Base — x:[3,12], y:[13,14]
    fill16(&mut buf, 3, 13, 12, 14, ST);

    buf.iter().flat_map(|p| *p).collect()
}

fn fill(buf: &mut Vec<[u8; 4]>, x0: usize, y0: usize, x1: usize, y1: usize, c: [u8; 4]) {
    for y in y0..=y1 { for x in x0..=x1 { buf[y * 32 + x] = c; } }
}
fn fill16(buf: &mut Vec<[u8; 4]>, x0: usize, y0: usize, x1: usize, y1: usize, c: [u8; 4]) {
    for y in y0..=y1 { for x in x0..=x1 { buf[y * 16 + x] = c; } }
}

// ── ICO encoding ───────────────────────────────────────────────────────────

fn build_ico() -> Vec<u8> {
    let img32 = rgba_to_bmp_dib(&render_32x32(), 32, 32);
    let img16 = rgba_to_bmp_dib(&render_16x16(), 16, 16);

    let count: u16 = 2;
    let dir_entry_size: u32 = 16;
    let header_size: u32 = 6 + dir_entry_size * count as u32;

    let offset32: u32 = header_size;
    let offset16: u32 = header_size + img32.len() as u32;

    let mut out = Vec::new();

    // ICO file header
    out.extend_from_slice(&[0x00, 0x00]); // reserved
    out.extend_from_slice(&[0x01, 0x00]); // type = icon
    out.extend_from_slice(&(count as u16).to_le_bytes());

    // Directory entry for 32×32
    ico_dir_entry(&mut out, 32, img32.len() as u32, offset32);
    // Directory entry for 16×16
    ico_dir_entry(&mut out, 16, img16.len() as u32, offset16);

    out.extend_from_slice(&img32);
    out.extend_from_slice(&img16);
    out
}

fn ico_dir_entry(out: &mut Vec<u8>, size: u8, data_len: u32, offset: u32) {
    out.push(size);            // width
    out.push(size);            // height
    out.push(0);               // colour count (0 = >256 colours)
    out.push(0);               // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&offset.to_le_bytes());
}

/// Encode RGBA pixels as a BITMAPINFOHEADER + XOR mask + AND mask (ICO sub-image).
/// Rows are written bottom-to-top (BMP convention).  Alpha is preserved.
fn rgba_to_bmp_dib(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();

    // BITMAPINFOHEADER (40 bytes)
    out.extend_from_slice(&40u32.to_le_bytes());           // biSize
    out.extend_from_slice(&(w as i32).to_le_bytes());      // biWidth
    out.extend_from_slice(&((h * 2) as i32).to_le_bytes()); // biHeight (×2 for XOR+AND)
    out.extend_from_slice(&1u16.to_le_bytes());            // biPlanes
    out.extend_from_slice(&32u16.to_le_bytes());           // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes());            // biCompression (BI_RGB)
    out.extend_from_slice(&0u32.to_le_bytes());            // biSizeImage (0 ok for BI_RGB)
    out.extend_from_slice(&0u32.to_le_bytes());            // biXPelsPerMeter
    out.extend_from_slice(&0u32.to_le_bytes());            // biYPelsPerMeter
    out.extend_from_slice(&0u32.to_le_bytes());            // biClrUsed
    out.extend_from_slice(&0u32.to_le_bytes());            // biClrImportant

    // XOR mask — 32-bit BGRA, rows bottom-to-top
    for y in (0..h as usize).rev() {
        for x in 0..w as usize {
            let i = (y * w as usize + x) * 4;
            out.push(rgba[i + 2]); // B
            out.push(rgba[i + 1]); // G
            out.push(rgba[i]);     // R
            out.push(rgba[i + 3]); // A
        }
    }

    // AND mask — 1-bit per pixel, rows bottom-to-top, row width padded to 4 bytes
    // All zero = use alpha channel for transparency (correct for 32-bit icons)
    let row_bytes = ((w + 31) / 32 * 4) as usize;
    for _ in 0..h as usize {
        for _ in 0..row_bytes {
            out.push(0x00);
        }
    }

    out
}
