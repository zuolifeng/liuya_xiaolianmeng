//! 六牙象·连萌：隐私提示标签 —— Windows 浮层窗口。
//!
//! 一个透明、无边框、置顶、不进任务栏的小窗口，用 tiny-skia 直接画一条提示。
//! 与 `whiteboard` 浮层的关键差别：
//!   - **不**调用 `set_ignore_cursor_events`，因为需要接收鼠标事件来拖动；
//!   - 窗口只有标签那么大，不覆盖整屏，不影响学生正常操作；
//!   - 拖动后的位置会被 clamp 在所有显示器的合并矩形内，拖不出屏幕；
//!   - 不响应关闭请求，只能由主进程通过 IPC 让它退出。

use super::{server::EVENT_PROXY, NoticeEvent};
use hbb_common::{
    anyhow::anyhow,
    config::LocalConfig,
    log, ResultType,
};
use softbuffer::{Context, Surface};
use std::{
    num::NonZeroU32,
    sync::Arc,
    time::{Duration, Instant},
};
use tao::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, Event, MouseButton, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::windows::WindowBuilderExtWindows,
    window::WindowBuilder,
};
use tiny_skia::{FillRule, Paint, PathBuilder, PixmapMut, Point, Rect, Transform};
use ttf_parser::Face;

/// 逻辑尺寸（后续按 DPI scale factor 放大）
const FONT_SIZE: f32 = 15.0;
const PADDING_X: f32 = 14.0;
const PADDING_Y: f32 = 9.0;
const DOT_RADIUS: f32 = 5.0;
const DOT_GAP: f32 = 9.0;
const CORNER_RADIUS: f32 = 8.0;
/// 默认落点距屏幕左上角的边距
const DEFAULT_MARGIN: i32 = 16;
/// 呼吸灯周期
const PULSE_PERIOD: Duration = Duration::from_millis(1600);
/// 事件循环空闲时的唤醒间隔：用来重新置顶 + 驱动呼吸灯
const TICK: Duration = Duration::from_millis(120);
/// 重新置顶的间隔（有些全屏程序会抢 topmost）
const REASSERT_TOPMOST: Duration = Duration::from_secs(2);

// 背景警示红 / 前景白
const BG: (u8, u8, u8) = (217, 48, 37);
const FG: (u8, u8, u8) = (255, 255, 255);

/// tiny-skia 画到 softbuffer 的缓冲区上时，字节序实际是 BGRA。
#[inline]
fn set_bgra(paint: &mut Paint, rgb: (u8, u8, u8), a: u8) {
    paint.set_color_rgba8(rgb.2, rgb.1, rgb.0, a);
}

// ---------------------------------------------------------------------------
// 字体
// ---------------------------------------------------------------------------

/// 优先挑一个能画中文的系统字体。
/// `whiteboard::win_linux::create_font_face` 只查 Monospace/SansSerif，
/// 在 Windows 上通常命中 Consolas / Segoe UI —— 它们没有汉字字形，
/// 直接用会把整条提示画成一串豆腐块。
fn create_cjk_font_face() -> ResultType<Face<'static>> {
    const CJK_FAMILIES: &[&str] = &[
        "Microsoft YaHei UI",
        "Microsoft YaHei",
        "微软雅黑",
        "SimHei",
        "黑体",
        "SimSun",
        "宋体",
        "Noto Sans CJK SC",
        "Source Han Sans SC",
        "Noto Sans SC",
    ];

    let mut font_db = fontdb::Database::new();
    font_db.load_system_fonts();

    let mut candidates: Vec<fontdb::ID> = Vec::new();
    for family in CJK_FAMILIES {
        let query = fontdb::Query {
            families: &[fontdb::Family::Name(family)],
            ..fontdb::Query::default()
        };
        if let Some(id) = font_db.query(&query) {
            candidates.push(id);
        }
    }
    // 兜底：系统默认无衬线 / 等宽
    let fallback = fontdb::Query {
        families: &[fontdb::Family::SansSerif, fontdb::Family::Monospace],
        ..fontdb::Query::default()
    };
    if let Some(id) = font_db.query(&fallback) {
        candidates.push(id);
    }

    for id in candidates {
        let Some((source, index)) = font_db.face_source(id) else {
            continue;
        };
        let data = match source {
            fontdb::Source::File(path) => std::fs::read(path).ok(),
            fontdb::Source::Binary(data) => Some(data.as_ref().as_ref().to_vec()),
            fontdb::Source::SharedFile(path, _) => std::fs::read(path).ok(),
        };
        let Some(data) = data else { continue };
        // ttf-parser 要求 'static，这里刻意 leak：字体要活到进程结束。
        let data: &'static [u8] = Box::leak(data.into_boxed_slice());
        let Ok(face) = Face::parse(data, index) else {
            continue;
        };
        // 抽查几个必用汉字，确认这个字体真的有中文字形
        if ['屏', '幕', '看', '私']
            .iter()
            .all(|c| face.glyph_index(*c).is_some())
        {
            return Ok(face);
        }
    }
    hbb_common::bail!("no CJK-capable font found");
}

struct TextMetrics {
    width: f32,
    height: f32,
    ascent: f32,
}

fn measure(face: &Face, text: &str, font_size: f32) -> TextMetrics {
    let units_per_em = face.units_per_em() as f32;
    let scale = font_size / units_per_em;
    let mut width = 0.0;
    for ch in text.chars() {
        let gid = face.glyph_index(ch).unwrap_or_default();
        if let Some(adv) = face.glyph_hor_advance(gid) {
            width += adv as f32 * scale;
        }
    }
    TextMetrics {
        width,
        height: (face.ascender() - face.descender()) as f32 * scale,
        ascent: face.ascender() as f32 * scale,
    }
}

struct GlyphSink<'a> {
    pb: &'a mut PathBuilder,
    transform: Transform,
}

impl ttf_parser::OutlineBuilder for GlyphSink<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let mut p = Point::from_xy(x, y);
        self.transform.map_point(&mut p);
        self.pb.move_to(p.x, p.y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let mut p = Point::from_xy(x, y);
        self.transform.map_point(&mut p);
        self.pb.line_to(p.x, p.y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let mut c = Point::from_xy(x1, y1);
        self.transform.map_point(&mut c);
        let mut p = Point::from_xy(x, y);
        self.transform.map_point(&mut p);
        self.pb.quad_to(c.x, c.y, p.x, p.y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let mut c1 = Point::from_xy(x1, y1);
        self.transform.map_point(&mut c1);
        let mut c2 = Point::from_xy(x2, y2);
        self.transform.map_point(&mut c2);
        let mut p = Point::from_xy(x, y);
        self.transform.map_point(&mut p);
        self.pb.cubic_to(c1.x, c1.y, c2.x, c2.y, p.x, p.y);
    }
    fn close(&mut self) {
        self.pb.close();
    }
}

fn draw_text(pixmap: &mut PixmapMut, face: &Face, text: &str, x: f32, y: f32, font_size: f32) {
    let units_per_em = face.units_per_em() as f32;
    let scale = font_size / units_per_em;
    // y 轴朝下，字形坐标系朝上，所以纵向取负。
    let base = Transform::from_translate(x, y).pre_scale(scale, -scale);
    let mut pb = PathBuilder::new();
    let mut cursor_x = 0.0;
    for ch in text.chars() {
        let gid = face.glyph_index(ch).unwrap_or_default();
        let mut sink = GlyphSink {
            pb: &mut pb,
            transform: base.post_translate(cursor_x, 0.0),
        };
        face.outline_glyph(gid, &mut sink);
        if let Some(adv) = face.glyph_hor_advance(gid) {
            cursor_x += adv as f32 * scale;
        }
    }
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        set_bgra(&mut paint, FG, 255);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

fn rounded_rect_path(rect: Rect, radius: f32) -> Option<tiny_skia::Path> {
    let (x, y, w, h) = (rect.x(), rect.y(), rect.width(), rect.height());
    let r = radius.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

// ---------------------------------------------------------------------------
// 位置：读写 + 夹取
// ---------------------------------------------------------------------------

fn load_saved_pos() -> Option<(i32, i32)> {
    let raw = LocalConfig::get_option(super::CONFIG_KEY_POS);
    let (x, y) = raw.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

fn save_pos(x: i32, y: i32) {
    LocalConfig::set_option(super::CONFIG_KEY_POS.to_owned(), format!("{},{}", x, y));
}

/// 把窗口位置夹回所有显示器的合并矩形内 —— 这就是"拖不出屏幕"。
fn clamp_pos(x: i32, y: i32, w: u32, h: u32, rect: (i32, i32, u32, u32)) -> (i32, i32) {
    let (rx, ry, rw, rh) = rect;
    let max_x = (rx + rw as i32 - w as i32).max(rx);
    let max_y = (ry + rh as i32 - h as i32).max(ry);
    (x.clamp(rx, max_x), y.clamp(ry, max_y))
}

// ---------------------------------------------------------------------------
// 事件循环
// ---------------------------------------------------------------------------

pub(super) fn create_event_loop() -> ResultType<()> {
    let face = create_cjk_font_face().map_err(|e| {
        log::error!("privacy notice: {}", e);
        e
    })?;

    let displays_rect = super::server::get_displays_rect().unwrap_or((0, 0, 1920, 1080));

    let event_loop = EventLoopBuilder::<NoticeEvent>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title("Lianmeng privacy notice")
        .with_transparent(true)
        .with_always_on_top(true)
        .with_skip_taskbar(true)
        .with_decorations(false)
        .with_resizable(false)
        // 先隐藏，等第一次 layout 算好尺寸和落点再显示，避免闪一下默认位置
        .with_visible(false)
        .with_inner_size(PhysicalSize::new(360u32, 40u32))
        .build::<NoticeEvent>(&event_loop)?;
    let window = Arc::new(window);

    let context = Context::new(window.clone()).map_err(|e| anyhow!(e.to_string()))?;
    let mut surface = Surface::new(&context, window.clone()).map_err(|e| anyhow!(e.to_string()))?;

    let proxy = event_loop.create_proxy();
    EVENT_PROXY.write().unwrap().replace(proxy);
    let _call_on_ret = crate::common::SimpleCallOnReturn {
        b: true,
        f: Box::new(move || {
            let _ = EVENT_PROXY.write().unwrap().take();
        }),
    };

    let mut text = super::build_notice_text(&[]);
    let mut needs_layout = true;
    // 拖动状态：按下时鼠标在窗口内的位置
    let mut drag_grab: Option<(f64, f64)> = None;
    let mut last_cursor: (f64, f64) = (0.0, 0.0);
    let mut last_topmost = Instant::now();
    let started = Instant::now();
    let mut placed = false;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + TICK);

        match event {
            // 不响应关闭请求 —— 提示条不可被学生关掉。
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {}

            Event::WindowEvent {
                event: WindowEvent::CursorMoved { position, .. },
                ..
            } => {
                last_cursor = (position.x, position.y);
                if let Some((gx, gy)) = drag_grab {
                    if let Ok(pos) = window.outer_position() {
                        let size = window.outer_size();
                        let nx = pos.x + (position.x - gx).round() as i32;
                        let ny = pos.y + (position.y - gy).round() as i32;
                        let (cx, cy) =
                            clamp_pos(nx, ny, size.width, size.height, displays_rect);
                        if (cx, cy) != (pos.x, pos.y) {
                            window.set_outer_position(PhysicalPosition::new(cx, cy));
                        }
                    }
                }
            }

            Event::WindowEvent {
                event:
                    WindowEvent::MouseInput {
                        state,
                        button: MouseButton::Left,
                        ..
                    },
                ..
            } => match state {
                ElementState::Pressed => {
                    drag_grab = Some(last_cursor);
                }
                _ => {
                    // Released / 其它状态：结束拖动并记住位置
                    drag_grab = None;
                    if let Ok(pos) = window.outer_position() {
                        save_pos(pos.x, pos.y);
                    }
                }
            },

            Event::WindowEvent {
                event: WindowEvent::CursorLeft { .. },
                ..
            } => {
                drag_grab = None;
            }

            Event::UserEvent(evt) => match evt {
                NoticeEvent::Viewers(viewers) => {
                    let next = super::build_notice_text(&viewers);
                    if next != text {
                        text = next;
                        needs_layout = true;
                        window.request_redraw();
                    }
                }
                NoticeEvent::Exit => {
                    *control_flow = ControlFlow::Exit;
                }
            },

            Event::MainEventsCleared => {
                // 定期重新置顶：防止被后来的全屏窗口盖住。
                if last_topmost.elapsed() >= REASSERT_TOPMOST {
                    last_topmost = Instant::now();
                    window.set_always_on_top(true);
                }
                // 呼吸灯需要持续重绘
                window.request_redraw();
            }

            Event::RedrawRequested(_) => {
                let scale = window.scale_factor() as f32;
                let font_size = FONT_SIZE * scale;
                let pad_x = PADDING_X * scale;
                let pad_y = PADDING_Y * scale;
                let dot_r = DOT_RADIUS * scale;
                let dot_gap = DOT_GAP * scale;

                let tm = measure(&face, &text, font_size);

                if needs_layout {
                    needs_layout = false;
                    let w = (pad_x * 2.0 + dot_r * 2.0 + dot_gap + tm.width).ceil() as u32;
                    let h = (pad_y * 2.0 + tm.height).ceil() as u32;
                    window.set_inner_size(PhysicalSize::new(w.max(1), h.max(1)));

                    if !placed {
                        placed = true;
                        let (x, y) = load_saved_pos().unwrap_or((
                            displays_rect.0 + DEFAULT_MARGIN,
                            displays_rect.1 + DEFAULT_MARGIN,
                        ));
                        let (cx, cy) = clamp_pos(x, y, w, h, displays_rect);
                        window.set_outer_position(PhysicalPosition::new(cx, cy));
                        window.set_visible(true);
                    } else if let Ok(pos) = window.outer_position() {
                        // 文案变长后可能超出右/下边界，重新夹一次
                        let (cx, cy) = clamp_pos(pos.x, pos.y, w, h, displays_rect);
                        if (cx, cy) != (pos.x, pos.y) {
                            window.set_outer_position(PhysicalPosition::new(cx, cy));
                        }
                    }
                    return;
                }

                let size = window.inner_size();
                let (Some(width), Some(height)) =
                    (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                else {
                    return;
                };
                if let Err(e) = surface.resize(width, height) {
                    log::error!("privacy notice: failed to resize surface: {}", e);
                    return;
                }
                let mut buffer = match surface.buffer_mut() {
                    Ok(b) => b,
                    Err(e) => {
                        log::error!("privacy notice: failed to get buffer: {}", e);
                        return;
                    }
                };
                let Some(mut pixmap) = PixmapMut::from_bytes(
                    bytemuck::cast_slice_mut(&mut buffer),
                    width.get(),
                    height.get(),
                ) else {
                    log::error!("privacy notice: failed to create pixmap");
                    return;
                };
                pixmap.fill(tiny_skia::Color::TRANSPARENT);

                let (w, h) = (width.get() as f32, height.get() as f32);

                // 背景圆角矩形
                if let Some(rect) = Rect::from_xywh(0.0, 0.0, w, h) {
                    if let Some(path) = rounded_rect_path(rect, CORNER_RADIUS * scale) {
                        let mut paint = Paint::default();
                        set_bgra(&mut paint, BG, 255);
                        paint.anti_alias = true;
                        pixmap.fill_path(
                            &path,
                            &paint,
                            FillRule::Winding,
                            Transform::identity(),
                            None,
                        );
                    }
                }

                // 呼吸的小圆点，提示"正在被查看"这件事是持续进行的
                let phase = (started.elapsed().as_millis() % PULSE_PERIOD.as_millis()) as f32
                    / PULSE_PERIOD.as_millis() as f32;
                let alpha = (0.45 + 0.55 * (1.0 - (phase * 2.0 - 1.0).abs())).clamp(0.0, 1.0);
                let mut dot_pb = PathBuilder::new();
                dot_pb.push_circle(pad_x + dot_r, h / 2.0, dot_r);
                if let Some(path) = dot_pb.finish() {
                    let mut paint = Paint::default();
                    set_bgra(&mut paint, FG, (alpha * 255.0) as u8);
                    paint.anti_alias = true;
                    pixmap.fill_path(
                        &path,
                        &paint,
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }

                // 文案
                let text_x = pad_x + dot_r * 2.0 + dot_gap;
                let text_y = (h - tm.height) / 2.0 + tm.ascent;
                draw_text(&mut pixmap, &face, &text, text_x, text_y, font_size);

                if let Err(e) = buffer.present() {
                    log::error!("privacy notice: failed to present: {}", e);
                }
            }
            _ => {}
        }
    });
}
