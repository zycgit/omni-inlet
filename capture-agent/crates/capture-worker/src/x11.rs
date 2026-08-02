use anyhow::{Context, Result, anyhow, bail};
use capture_protocol::{CaptureSourceInfo, CaptureSourceKind};
use x11rb::{
    connection::Connection,
    protocol::xproto::{
        Atom, AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder, MapState, Visualtype, Window,
    },
    rust_connection::RustConnection,
};

use crate::source::{CaptureSource, CapturedFrame, SourceState};

#[derive(Clone, Copy)]
struct Atoms {
    net_client_list: Atom,
    net_wm_name: Atom,
    net_wm_pid: Atom,
    utf8_string: Atom,
}

pub struct X11WindowInfo {
    pub source: CaptureSourceInfo,
    pub application_name: String,
    pub process_id: Option<u32>,
}

pub struct X11WindowSource {
    connection: RustConnection,
    window: Window,
    info: CaptureSourceInfo,
    visual: Visualtype,
    bits_per_pixel: u8,
    image_byte_order: ImageOrder,
}

impl X11WindowSource {
    pub fn connect(window: Window) -> Result<Self> {
        let (connection, screen_number) =
            RustConnection::connect(None).context("cannot connect to the X11 display")?;
        let atoms = intern_atoms(&connection)?;
        let attributes = connection
            .get_window_attributes(window)?
            .reply()
            .with_context(|| format!("cannot read attributes for X11 window 0x{window:x}"))?;
        let geometry = connection
            .get_geometry(window)?
            .reply()
            .with_context(|| format!("cannot read geometry for X11 window 0x{window:x}"))?;
        let title = window_title(&connection, window, atoms)
            .unwrap_or_else(|_| format!("X11 window 0x{window:x}"));
        let setup = connection.setup();
        let screen = setup
            .roots
            .get(screen_number)
            .context("X11 screen index is invalid")?;
        let visual = find_visual(setup, attributes.visual)
            .or_else(|| find_visual(setup, screen.root_visual))
            .context("cannot find the X11 visual used by the target window")?;
        let bits_per_pixel = setup
            .pixmap_formats
            .iter()
            .find(|format| format.depth == geometry.depth)
            .map(|format| format.bits_per_pixel)
            .with_context(|| {
                format!(
                    "unsupported X11 depth {}; no pixmap format was advertised",
                    geometry.depth
                )
            })?;
        let image_byte_order = setup.image_byte_order;

        if bits_per_pixel != 24 && bits_per_pixel != 32 {
            bail!("unsupported X11 pixel size: {bits_per_pixel} bits per pixel");
        }

        Ok(Self {
            connection,
            window,
            info: CaptureSourceInfo {
                kind: CaptureSourceKind::X11Window,
                id: format!("0x{window:x}"),
                title,
                width: geometry.width.into(),
                height: geometry.height.into(),
                visible: attributes.map_state == MapState::VIEWABLE,
            },
            visual,
            bits_per_pixel,
            image_byte_order,
        })
    }
}

impl CaptureSource for X11WindowSource {
    fn info(&self) -> CaptureSourceInfo {
        self.info.clone()
    }

    fn capture(&mut self) -> Result<CapturedFrame> {
        let geometry = self
            .connection
            .get_geometry(self.window)?
            .reply()
            .context("target X11 window is no longer available")?;
        let width = u32::from(geometry.width);
        let height = u32::from(geometry.height);
        if width == 0 || height == 0 {
            bail!("target X11 window has an invalid size: {width}x{height}");
        }

        let reply = self
            .connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.window,
                0,
                0,
                geometry.width,
                geometry.height,
                u32::MAX,
            )?
            .reply()
            .context("X11 GetImage failed; the window may be minimized or closed")?;
        let rgba = decode_z_pixmap(
            &reply.data,
            width,
            height,
            self.bits_per_pixel,
            self.image_byte_order,
            &self.visual,
        )?;

        self.info.width = width;
        self.info.height = height;

        Ok(CapturedFrame {
            width,
            height,
            rgba,
        })
    }

    fn state(&self) -> Result<SourceState> {
        let attributes = match self.connection.get_window_attributes(self.window) {
            Ok(cookie) => match cookie.reply() {
                Ok(attributes) => attributes,
                Err(_) => return Ok(SourceState::Destroyed),
            },
            Err(_) => return Ok(SourceState::Destroyed),
        };
        Ok(if attributes.map_state == MapState::VIEWABLE {
            SourceState::Available
        } else {
            SourceState::Hidden
        })
    }
}

pub fn list_windows() -> Result<Vec<CaptureSourceInfo>> {
    Ok(list_window_details()?
        .into_iter()
        .map(|window| window.source)
        .collect())
}

pub fn list_window_details() -> Result<Vec<X11WindowInfo>> {
    let (connection, screen_number) =
        RustConnection::connect(None).context("cannot connect to the X11 display")?;
    let screen = connection
        .setup()
        .roots
        .get(screen_number)
        .context("X11 screen index is invalid")?;
    let atoms = intern_atoms(&connection)?;

    let windows = client_windows(&connection, screen.root, atoms).unwrap_or_else(|_| {
        connection
            .query_tree(screen.root)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| reply.children)
            .unwrap_or_default()
    });

    let mut result = Vec::new();
    for window in windows {
        let Ok(attributes_cookie) = connection.get_window_attributes(window) else {
            continue;
        };
        let Ok(attributes) = attributes_cookie.reply() else {
            continue;
        };
        let Ok(geometry_cookie) = connection.get_geometry(window) else {
            continue;
        };
        let Ok(geometry) = geometry_cookie.reply() else {
            continue;
        };
        if geometry.width == 0 || geometry.height == 0 {
            continue;
        }
        let title = window_title(&connection, window, atoms).unwrap_or_default();
        if title.trim().is_empty() {
            continue;
        }
        result.push(X11WindowInfo {
            source: CaptureSourceInfo {
                kind: CaptureSourceKind::X11Window,
                id: format!("0x{window:x}"),
                title,
                width: geometry.width.into(),
                height: geometry.height.into(),
                visible: attributes.map_state == MapState::VIEWABLE,
            },
            application_name: window_class(&connection, window)
                .unwrap_or_else(|_| "X11 应用".to_string()),
            process_id: window_pid(&connection, window, atoms).ok(),
        });
    }
    result.sort_by(|left, right| {
        left.application_name
            .cmp(&right.application_name)
            .then(left.source.title.cmp(&right.source.title))
    });
    Ok(result)
}

pub fn parse_window_id(value: &str) -> Result<Window> {
    let value = value.trim();
    let parsed = if let Some(hex) = value.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else {
        value.parse::<u32>()
    };
    parsed.map_err(|error| anyhow!("invalid X11 window id '{value}': {error}"))
}

fn intern_atoms(connection: &RustConnection) -> Result<Atoms> {
    Ok(Atoms {
        net_client_list: connection
            .intern_atom(false, b"_NET_CLIENT_LIST")?
            .reply()?
            .atom,
        net_wm_name: connection
            .intern_atom(false, b"_NET_WM_NAME")?
            .reply()?
            .atom,
        net_wm_pid: connection.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom,
        utf8_string: connection.intern_atom(false, b"UTF8_STRING")?.reply()?.atom,
    })
}

fn window_class(connection: &RustConnection, window: Window) -> Result<String> {
    let reply = connection
        .get_property(
            false,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            0,
            u32::MAX,
        )?
        .reply()?;
    let names = reply
        .value
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(String::from_utf8_lossy)
        .collect::<Vec<_>>();
    names
        .last()
        .map(|value| value.to_string())
        .context("WM_CLASS is empty")
}

fn window_pid(connection: &RustConnection, window: Window, atoms: Atoms) -> Result<u32> {
    connection
        .get_property(false, window, atoms.net_wm_pid, AtomEnum::CARDINAL, 0, 1)?
        .reply()?
        .value32()
        .and_then(|mut values| values.next())
        .context("_NET_WM_PID is unavailable")
}

fn client_windows(connection: &RustConnection, root: Window, atoms: Atoms) -> Result<Vec<Window>> {
    let reply = connection
        .get_property(
            false,
            root,
            atoms.net_client_list,
            AtomEnum::WINDOW,
            0,
            u32::MAX,
        )?
        .reply()?;
    reply
        .value32()
        .map(|values| values.collect())
        .context("_NET_CLIENT_LIST is not a 32-bit window list")
}

fn window_title(connection: &RustConnection, window: Window, atoms: Atoms) -> Result<String> {
    let utf8 = connection
        .get_property(
            false,
            window,
            atoms.net_wm_name,
            atoms.utf8_string,
            0,
            u32::MAX,
        )?
        .reply()?;
    if !utf8.value.is_empty() {
        return Ok(String::from_utf8_lossy(&utf8.value)
            .trim_end_matches('\0')
            .to_string());
    }

    let legacy = connection
        .get_property(
            false,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            0,
            u32::MAX,
        )?
        .reply()?;
    Ok(String::from_utf8_lossy(&legacy.value)
        .trim_end_matches('\0')
        .to_string())
}

fn find_visual(setup: &x11rb::protocol::xproto::Setup, visual_id: u32) -> Option<Visualtype> {
    setup
        .roots
        .iter()
        .flat_map(|screen| &screen.allowed_depths)
        .flat_map(|depth| &depth.visuals)
        .find(|visual| visual.visual_id == visual_id)
        .cloned()
}

fn decode_z_pixmap(
    data: &[u8],
    width: u32,
    height: u32,
    bits_per_pixel: u8,
    image_byte_order: ImageOrder,
    visual: &Visualtype,
) -> Result<Vec<u8>> {
    let bytes_per_pixel = usize::from(bits_per_pixel / 8);
    let height_usize = usize::try_from(height)?;
    let width_usize = usize::try_from(width)?;
    if height_usize == 0 || data.len() % height_usize != 0 {
        bail!("X11 returned an invalid pixel buffer length");
    }
    let row_stride = data.len() / height_usize;
    if row_stride < width_usize * bytes_per_pixel {
        bail!("X11 pixel buffer row is smaller than the requested width");
    }

    let mut rgba = vec![0_u8; width_usize * height_usize * 4];
    for y in 0..height_usize {
        for x in 0..width_usize {
            let input = y * row_stride + x * bytes_per_pixel;
            let pixel = match (bytes_per_pixel, image_byte_order) {
                (4, ImageOrder::LSB_FIRST) => u32::from_le_bytes([
                    data[input],
                    data[input + 1],
                    data[input + 2],
                    data[input + 3],
                ]),
                (4, ImageOrder::MSB_FIRST) => u32::from_be_bytes([
                    data[input],
                    data[input + 1],
                    data[input + 2],
                    data[input + 3],
                ]),
                (3, ImageOrder::LSB_FIRST) => {
                    u32::from_le_bytes([data[input], data[input + 1], data[input + 2], 0])
                }
                (3, ImageOrder::MSB_FIRST) => {
                    u32::from_be_bytes([0, data[input], data[input + 1], data[input + 2]])
                }
                _ => unreachable!("bits per pixel was checked during source creation"),
            };
            let output = (y * width_usize + x) * 4;
            rgba[output] = component(pixel, visual.red_mask);
            rgba[output + 1] = component(pixel, visual.green_mask);
            rgba[output + 2] = component(pixel, visual.blue_mask);
            rgba[output + 3] = 255;
        }
    }
    Ok(rgba)
}

fn component(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let value = (pixel & mask) >> shift;
    let maximum = mask >> shift;
    ((u64::from(value) * 255) / u64::from(maximum)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visual() -> Visualtype {
        Visualtype {
            visual_id: 1,
            class: x11rb::protocol::xproto::VisualClass::TRUE_COLOR,
            bits_per_rgb_value: 8,
            colormap_entries: 256,
            red_mask: 0x00ff0000,
            green_mask: 0x0000ff00,
            blue_mask: 0x000000ff,
        }
    }

    #[test]
    fn parses_decimal_and_hex_window_ids() {
        assert_eq!(parse_window_id("42").unwrap(), 42);
        assert_eq!(parse_window_id("0x2a").unwrap(), 42);
        assert!(parse_window_id("window").is_err());
    }

    #[test]
    fn decodes_common_little_endian_bgrx_buffer() {
        let rgba = decode_z_pixmap(
            &[0x33, 0x22, 0x11, 0],
            1,
            1,
            32,
            ImageOrder::LSB_FIRST,
            &visual(),
        )
        .unwrap();
        assert_eq!(rgba, vec![0x11, 0x22, 0x33, 255]);
    }
}
