//! The annotation editor: a window over the captured frame where the user
//! marks it up, rendered with the same wgpu setup as the region selector.
//!
//! The editor owns **no** annotation state of its own — every committed
//! edit, including undo and redo, goes through
//! [`foxshot_core::AnnotationDocument`]. What the user sees is the CPU
//! composite from [`crate::flatten`], uploaded to the capture texture
//! whenever the document or the in-progress mark changes, so the picture on
//! screen is byte-identical to what save or copy flattens.

use crate::digits::GLYPH_WIDTH;
use crate::flatten;
use crate::overlay::{self, OverlayVertex, Quads};
use crate::selector::UiError;
use foxshot_core::annotation::{AnnotationDocument, Ink, Mark, MarkId, MarkKind};
use foxshot_core::frame::Frame;
use foxshot_core::geometry::{Point, Rect};
use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Modifiers, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowLevel};

/// How the editor session ended. `Saved` and `Copied` carry the flattened
/// frame — the capture with every mark composited — produced only now, at
/// the moment of save or copy. The document's own frame was never touched.
#[derive(Debug)]
pub enum EditorOutcome {
    /// The user asked to save (Ctrl+S); the flattened frame is inside.
    Saved(Frame),
    /// The user asked to copy (Ctrl+C); the flattened frame is inside.
    Copied(Frame),
    /// The user cancelled (Escape); nothing was produced.
    Cancelled,
}

/// The annotation editor.
///
/// [`Editor::run`] opens a window showing the capture unscaled and centred,
/// with a tool rail down the left edge and the live findings count in the
/// top-right corner. Tools: **V** select, **R** rectangle, **A** arrow,
/// **T** text (click places a caret, typing appends, Enter commits),
/// **N** step number, **F** blur; **1**–**6** pick the ink colour;
/// Ctrl+Z / Ctrl+Shift+Z undo and redo through the document; Ctrl+S saves,
/// Ctrl+C copies, Escape cancels.
#[derive(Debug)]
pub struct Editor;

impl Editor {
    /// Runs the editor modally over `frame` and returns how it ended.
    pub fn run(frame: Frame) -> Result<EditorOutcome, UiError> {
        let event_loop =
            EventLoop::new().map_err(|e| UiError::Windowing(format!("event loop: {e}")))?;
        event_loop.set_control_flow(ControlFlow::Wait);
        let mut app = EditorApp::new(frame);
        event_loop
            .run_app(&mut app)
            .map_err(|e| UiError::Windowing(format!("run: {e}")))?;
        Ok(app.outcome.unwrap_or(EditorOutcome::Cancelled))
    }
}

/// The ink palette, chosen with keys 1–6. The default ink is entry 0.
const PALETTE: [[u8; 4]; 6] = [
    [0xFF, 0x6A, 0x3D, 0xFF],
    [0xE0, 0x57, 0x4C, 0xFF],
    [0xD9, 0xA0, 0x36, 0xFF],
    [0x63, 0xB9, 0x8C, 0xFF],
    [0x4C, 0x8F, 0xD9, 0xFF],
    [0xF7, 0xEF, 0xE6, 0xFF],
];

/// Width of the tool rail in pixels.
const RAIL_W: i32 = 44;
/// Padding around the capture in pixels.
const PAD: i32 = 8;
/// Text mark size in points (one bitmap cell per 5 points).
const TEXT_SIZE: u16 = 16;
/// Step-number badge diameter in pixels.
const STEP_DIAMETER: u32 = 24;
/// Blur radius of a fresh blur mark, in pixels.
const BLUR_RADIUS: u8 = 6;

/// The tool the keyboard and mouse currently create marks with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Rectangle,
    Arrow,
    Text,
    StepNumber,
    Blur,
}

impl Tool {
    /// Every tool in rail order, with its key and rail glyph.
    const ALL: [(Tool, char); 6] = [
        (Tool::Select, 'v'),
        (Tool::Rectangle, 'r'),
        (Tool::Arrow, 'a'),
        (Tool::Text, 't'),
        (Tool::StepNumber, 'n'),
        (Tool::Blur, 'f'),
    ];
}

/// An in-progress drag in image coordinates.
#[derive(Debug, Clone, Copy)]
struct Drag {
    start: Point,
    current: Point,
}

/// An in-progress text mark: caret position plus what was typed so far.
#[derive(Debug, Clone)]
struct TextEdit {
    origin: Point,
    content: String,
}

/// The winit application: window plus GPU state plus the document.
struct EditorApp {
    doc: AnnotationDocument,
    tool: Tool,
    ink: Ink,
    cursor: Point,
    drag: Option<Drag>,
    text: Option<TextEdit>,
    selected: Option<MarkId>,
    modifiers: Modifiers,
    outcome: Option<EditorOutcome>,
    composite_dirty: bool,
    surface_size: (u32, u32),
    gpu: Option<Gpu>,
}

impl EditorApp {
    fn new(frame: Frame) -> Self {
        let frame_size = frame.size();
        Self {
            doc: AnnotationDocument::new(frame),
            tool: Tool::Rectangle,
            ink: Ink::new(PALETTE[0], 3),
            cursor: Point { x: 0, y: 0 },
            drag: None,
            text: None,
            selected: None,
            modifiers: Modifiers::default(),
            outcome: None,
            composite_dirty: true,
            surface_size: (
                (RAIL_W + frame_size.width as i32 + 2 * PAD).max(1) as u32,
                (frame_size.height as i32 + 2 * PAD).max(1) as u32,
            ),
            gpu: None,
        }
    }

    /// The capture's top-left corner in surface pixels: centred in the
    /// window area right of the rail.
    fn capture_offset(&self) -> Point {
        let (fw, fh) = {
            let size = self.doc.frame().size();
            (size.width as i32, size.height as i32)
        };
        let (ww, wh) = (self.surface_size.0 as i32, self.surface_size.1 as i32);
        Point {
            x: RAIL_W + (ww - RAIL_W - fw).max(0) / 2,
            y: (wh - fh).max(0) / 2,
        }
    }

    /// Window coordinates to image coordinates, clamped into the image.
    fn image_point(&self, window: Point) -> Point {
        let offset = self.capture_offset();
        let size = self.doc.frame().size();
        Point {
            x: (window.x - offset.x).clamp(0, size.width as i32 - 1),
            y: (window.y - offset.y).clamp(0, size.height as i32 - 1),
        }
    }

    /// True when the window point sits over the capture, not the rail.
    fn over_capture(&self, window: Point) -> bool {
        let offset = self.capture_offset();
        let size = self.doc.frame().size();
        window.x >= offset.x
            && window.y >= offset.y
            && window.x < offset.x + size.width as i32
            && window.y < offset.y + size.height as i32
    }

    /// The mark a drag-in-progress would commit if released now.
    fn preview_mark(&self) -> Option<Mark> {
        let drag = self.drag?;
        let kind = match self.tool {
            Tool::Rectangle => MarkKind::Rectangle,
            Tool::Arrow => {
                MarkKind::Arrow { from: drag.start, to: drag.current }
            }
            Tool::Blur => MarkKind::Blur { radius: BLUR_RADIUS },
            _ => return None,
        };
        Some(Mark { id: MarkId(0), kind, bounds: drag_bounds(&drag), ink: self.ink })
    }

    /// The composite the user sees: the frame plus every committed mark
    /// plus the in-progress mark (drag preview, uncommitted text, the
    /// selection outline).
    fn composite(&self) -> Vec<u8> {
        let frame = self.doc.frame();
        let mut pixels = frame.bytes().to_vec();
        for mark in self.doc.marks() {
            flatten::draw_mark(&mut pixels, frame.size(), mark);
        }
        if let Some(preview) = self.preview_mark() {
            flatten::draw_mark(&mut pixels, frame.size(), &preview);
        }
        if let Some(text) = &self.text {
            if !text.content.is_empty() {
                flatten::draw_mark(&mut pixels, frame.size(), &text_mark(text, self.ink));
            }
        }
        if let Some(id) = self.selected {
            if let Some(mark) = self.doc.marks().iter().find(|m| m.id == id) {
                // A white bounding outline marks the selected mark. `Crop`
                // is a kind this slice does not draw, so it renders as
                // exactly that: its bounding outline.
                let marker = Mark {
                    id: MarkId(0),
                    kind: MarkKind::Crop,
                    bounds: mark.bounds,
                    ink: Ink::new([0xFF, 0xFF, 0xFF, 0xFF], 1),
                };
                flatten::draw_mark(&mut pixels, frame.size(), &marker);
            }
        }
        pixels
    }

    /// Commits the in-progress text mark into the document, if non-empty.
    fn commit_text(&mut self) {
        if let Some(text) = self.text.take() {
            if !text.content.is_empty() {
                let mark = text_mark(&text, self.ink);
                self.doc.add(mark.kind, mark.bounds, mark.ink);
                self.composite_dirty = true;
            }
        }
    }

    /// Commits the drag-in-progress as a mark, when it is big enough to
    /// be a mark at all (a click is not a rectangle).
    fn commit_drag(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        let bounds = drag_bounds(&drag);
        let add = match self.tool {
            Tool::Rectangle => bounds.size.width >= 2 && bounds.size.height >= 2,
            Tool::Blur => bounds.size.width >= 2 && bounds.size.height >= 2,
            Tool::Arrow => {
                let (dx, dy) = (drag.current.x - drag.start.x, drag.current.y - drag.start.y);
                dx * dx + dy * dy >= 16
            }
            _ => false,
        };
        if add {
            let kind = match self.tool {
                Tool::Rectangle => MarkKind::Rectangle,
                Tool::Blur => MarkKind::Blur { radius: BLUR_RADIUS },
                Tool::Arrow => MarkKind::Arrow { from: drag.start, to: drag.current },
                _ => unreachable!("add is only true for drag tools"),
            };
            self.doc.add(kind, bounds, self.ink);
        }
        self.composite_dirty = true;
    }

    /// Flattens the document (committing any in-progress text first) and
    /// exits with `Saved` or `Copied`.
    fn finish(&mut self, event_loop: &ActiveEventLoop, copy: bool) {
        self.commit_text();
        let flattened = flatten::flatten(&self.doc);
        self.outcome =
            Some(if copy { EditorOutcome::Copied(flattened) } else { EditorOutcome::Saved(flattened) });
        event_loop.exit();
    }

    fn on_key(&mut self, event_loop: &ActiveEventLoop, event: &winit::event::KeyEvent) {
        let ctrl = self.modifiers.state().control_key();
        let shift = self.modifiers.state().shift_key();
        if ctrl {
            match &event.logical_key {
                Key::Character(c) if c.eq_ignore_ascii_case("s") => {
                    self.finish(event_loop, false);
                }
                Key::Character(c) if c.eq_ignore_ascii_case("c") => {
                    self.finish(event_loop, true);
                }
                Key::Character(c) if c.eq_ignore_ascii_case("z") && !event.repeat => {
                    if shift { self.doc.redo(); } else { self.doc.undo(); }
                    self.selected = None;
                    self.composite_dirty = true;
                }
                _ => {}
            }
            return;
        }
        // While a text caret is active, keys type into the text mark.
        if self.tool == Tool::Text && self.text.is_some() {
            match &event.logical_key {
                Key::Named(NamedKey::Enter) => self.commit_text(),
                Key::Named(NamedKey::Escape) => {
                    self.outcome = Some(EditorOutcome::Cancelled);
                    event_loop.exit();
                }
                Key::Named(NamedKey::Backspace) => {
                    if let Some(text) = &mut self.text {
                        text.content.pop();
                        self.composite_dirty = true;
                    }
                }
                Key::Character(c) => {
                    if let Some(text) = &mut self.text {
                        for ch in c.chars().filter(|ch| !ch.is_control()) {
                            text.content.push(ch);
                        }
                        self.composite_dirty = true;
                    }
                }
                _ => {}
            }
            return;
        }
        if event.repeat {
            return;
        }
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.outcome = Some(EditorOutcome::Cancelled);
                event_loop.exit();
            }
            Key::Named(NamedKey::Delete) | Key::Named(NamedKey::Backspace) => {
                if let Some(id) = self.selected.take() {
                    self.doc.remove(id);
                    self.composite_dirty = true;
                }
            }
            Key::Character(c) => {
                let c = c.as_str();
                if let Some((tool, _)) =
                    Tool::ALL.iter().find(|(_, key)| c.eq_ignore_ascii_case(&key.to_string()))
                {
                    let tool = *tool;
                    if self.tool == Tool::Text && tool != Tool::Text {
                        self.commit_text();
                    }
                    self.tool = tool;
                } else if let Some(index) = c.parse::<usize>().ok().filter(|i| (1..=6).contains(i))
                {
                    self.ink.colour = PALETTE[index - 1];
                }
            }
            _ => {}
        }
    }
}

/// The normalized bounding box of a drag (from/arrow keep their own points;
/// this box is the mark's bounds).
fn drag_bounds(drag: &Drag) -> Rect {
    let left = drag.start.x.min(drag.current.x);
    let top = drag.start.y.min(drag.current.y);
    let right = drag.start.x.max(drag.current.x);
    let bottom = drag.start.y.max(drag.current.y);
    Rect::from_xywh(left, top, (right - left) as u32, (bottom - top) as u32)
}

/// Builds the (uncommitted) mark a [`TextEdit`] represents, for preview and
/// for committing.
fn text_mark(text: &TextEdit, ink: Ink) -> Mark {
    Mark {
        id: MarkId(0),
        kind: MarkKind::Text { content: text.content.clone(), size: TEXT_SIZE },
        bounds: Rect::from_xywh(
            text.origin.x,
            text.origin.y,
            flatten::text_width(&text.content, TEXT_SIZE).max(1) as u32,
            15,
        ),
        ink,
    }
}

impl ApplicationHandler for EditorApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        match Gpu::new(event_loop, self.doc.frame()) {
            Ok(gpu) => {
                gpu.window.request_redraw();
                self.gpu = Some(gpu);
            }
            Err(error) => {
                eprintln!("foxshot-ui: {error}");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.gpu.as_ref().map(|gpu| gpu.window.clone()) else { return };
        match event {
            WindowEvent::CloseRequested => {
                self.outcome = Some(EditorOutcome::Cancelled);
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                self.surface_size = (size.width, size.height);
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Point { x: position.x as i32, y: position.y as i32 };
                if self.drag.is_some() {
                    let point = self.image_point(self.cursor);
                    if let Some(drag) = &mut self.drag {
                        drag.current = point;
                    }
                    self.composite_dirty = true;
                }
                window.request_redraw();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                match state {
                    ElementState::Pressed => {
                        if self.over_capture(self.cursor) {
                            let point = self.image_point(self.cursor);
                            match self.tool {
                                Tool::Select => {
                                    self.selected = self.doc.mark_at(point);
                                    self.composite_dirty = true;
                                }
                                Tool::Rectangle | Tool::Arrow | Tool::Blur => {
                                    self.drag = Some(Drag { start: point, current: point });
                                    self.composite_dirty = true;
                                }
                                Tool::StepNumber => {
                                    self.doc.add(
                                        MarkKind::StepNumber { index: 0 },
                                        Rect::from_xywh(
                                            point.x,
                                            point.y,
                                            STEP_DIAMETER,
                                            STEP_DIAMETER,
                                        ),
                                        self.ink,
                                    );
                                    self.composite_dirty = true;
                                }
                                Tool::Text => {
                                    self.commit_text();
                                    self.text =
                                        Some(TextEdit { origin: point, content: String::new() });
                                }
                            }
                        }
                    }
                    ElementState::Released => self.commit_drag(),
                }
                window.request_redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                self.on_key(event_loop, &event);
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if self.composite_dirty {
                    let pixels = self.composite();
                    if let Some(gpu) = &mut self.gpu {
                        gpu.upload_capture(&pixels);
                    }
                    self.composite_dirty = false;
                }
                let offset = self.capture_offset();
                let quads = build_overlay(self);
                if let Some(gpu) = &mut self.gpu {
                    gpu.render(offset, &quads);
                }
            }
            _ => {}
        }
    }
}

/// Scene uniform for the capture shader: where the capture sits in the
/// surface (pixels) and the surface size.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SceneUniform {
    dst: [f32; 4],
    viewport: [f32; 2],
    _pad: [f32; 2],
}

/// Overlay uniform: the surface size, for pixel-to-clip conversion.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct OverlayUniform {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

/// Everything wgpu needs to draw the editor window.
struct Gpu {
    window: Arc<Window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    capture_texture: wgpu::Texture,
    capture_pipeline: wgpu::RenderPipeline,
    capture_bind_group: wgpu::BindGroup,
    scene_uniform: wgpu::Buffer,
    overlay_pipeline: wgpu::RenderPipeline,
    overlay_bind_group: wgpu::BindGroup,
    overlay_uniform: wgpu::Buffer,
    overlay_vertices: wgpu::Buffer,
}

impl fmt::Debug for Gpu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gpu").field("config", &self.config).finish_non_exhaustive()
    }
}

impl Gpu {
    /// Creates the window and all GPU state, and uploads `frame` as the
    /// capture texture.
    fn new(event_loop: &ActiveEventLoop, frame: &Frame) -> Result<Self, UiError> {
        let size = frame.size();
        let attributes = Window::default_attributes()
            .with_title("FoxShot — edit capture")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(winit::dpi::PhysicalPosition::new(0, 0))
            .with_inner_size(winit::dpi::PhysicalSize::new(
                (RAIL_W + size.width as i32 + 2 * PAD).max(1),
                (size.height as i32 + 2 * PAD).max(1),
            ));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|e| UiError::Windowing(format!("create window: {e}")))?,
        );

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| UiError::Gpu(format!("surface: {e}")))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .map_err(|e| UiError::Gpu(format!("no adapter: {e}")))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|e| UiError::Gpu(format!("no device: {e}")))?;

        let window_size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: window_size.width.max(1),
            height: window_size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let capture_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture"),
            size: wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texture_view = capture_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let scene_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene uniform"),
            size: std::mem::size_of::<SceneUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let capture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("capture layout"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                uniform_entry(2, wgpu::ShaderStages::VERTEX),
            ],
        });
        let capture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("capture bind group"),
            layout: &capture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry { binding: 2, resource: scene_uniform.as_entire_binding() },
            ],
        });
        let capture_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("capture shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(CAPTURE_WGSL)),
        });
        let capture_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("capture pipeline"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("capture pipeline layout"),
                    bind_group_layouts: &[&capture_layout],
                    push_constant_ranges: &[],
                }),
            ),
            vertex: wgpu::VertexState {
                module: &capture_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &capture_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let overlay_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay uniform"),
            size: std::mem::size_of::<OverlayUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let overlay_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay layout"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX)],
        });
        let overlay_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay bind group"),
            layout: &overlay_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: overlay_uniform.as_entire_binding(),
            }],
        });
        let overlay_vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay vertices"),
            size: (overlay::MAX_VERTICES * std::mem::size_of::<OverlayVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let overlay_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(overlay::WGSL)),
        });
        let overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay pipeline"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("overlay pipeline layout"),
                    bind_group_layouts: &[&overlay_layout],
                    push_constant_ranges: &[],
                }),
            ),
            vertex: wgpu::VertexState {
                module: &overlay_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<OverlayVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 8,
                            shader_location: 1,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &overlay_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let gpu = Self {
            window,
            device,
            queue,
            surface,
            config,
            capture_texture,
            capture_pipeline,
            capture_bind_group,
            scene_uniform,
            overlay_pipeline,
            overlay_bind_group,
            overlay_uniform,
            overlay_vertices,
        };
        gpu.upload_capture(frame.bytes());
        Ok(gpu)
    }

    /// Reconfigures the surface after a resize.
    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    /// Replaces the capture texture contents with the current composite.
    fn upload_capture(&self, pixels: &[u8]) {
        let size = self.capture_texture.size();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.capture_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width * 4),
                rows_per_image: Some(size.height),
            },
            size,
        );
    }

    /// Draws one frame: the capture quad at its offset, then the overlay
    /// quads (tool rail, findings count, caret) on top.
    fn render(&mut self, offset: Point, overlay_quads: &Quads) {
        let capture_size = self.capture_texture.size();
        self.queue.write_buffer(
            &self.scene_uniform,
            0,
            bytemuck::bytes_of(&SceneUniform {
                dst: [
                    offset.x as f32,
                    offset.y as f32,
                    capture_size.width as f32,
                    capture_size.height as f32,
                ],
                viewport: [self.config.width as f32, self.config.height as f32],
                _pad: [0.0; 2],
            }),
        );

        let vertices = overlay::to_vertices(overlay_quads);
        if !vertices.is_empty() {
            self.queue.write_buffer(
                &self.overlay_vertices,
                0,
                bytemuck::cast_slice(&vertices),
            );
        }
        self.queue.write_buffer(
            &self.overlay_uniform,
            0,
            bytemuck::bytes_of(&OverlayUniform {
                viewport: [self.config.width as f32, self.config.height as f32],
                _pad: [0.0; 2],
            }),
        );

        let target = match self.surface.get_current_texture() {
            Ok(target) => target,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(error) => {
                eprintln!("foxshot-ui: frame acquire failed: {error}");
                return;
            }
        };
        let view = target.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("editor") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("editor pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.08,
                            g: 0.08,
                            b: 0.09,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.capture_pipeline);
            pass.set_bind_group(0, &self.capture_bind_group, &[]);
            pass.draw(0..6, 0..1);
            if !vertices.is_empty() {
                pass.set_pipeline(&self.overlay_pipeline);
                pass.set_bind_group(0, &self.overlay_bind_group, &[]);
                pass.set_vertex_buffer(0, self.overlay_vertices.slice(..));
                pass.draw(0..vertices.len() as u32, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        target.present();
    }
}

/// Ink bytes to linear-ish RGBA floats for the overlay shader.
fn ink_floats(colour: [u8; 4]) -> [f32; 4] {
    [
        f32::from(colour[0]) / 255.0,
        f32::from(colour[1]) / 255.0,
        f32::from(colour[2]) / 255.0,
        f32::from(colour[3]) / 255.0,
    ]
}

/// Builds the overlay quads: the tool rail down the left edge (active tool
/// highlighted), the ink palette swatches, the live findings count in the
/// top-right corner and the text caret when a text edit is in progress.
fn build_overlay(app: &EditorApp) -> Quads {
    const RAIL_BG: [f32; 4] = [0.10, 0.10, 0.12, 0.95];
    const TOOL_BG: [f32; 4] = [0.22, 0.22, 0.25, 0.9];
    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    const COUNT_BG: [f32; 4] = [0.0, 0.0, 0.0, 0.6];

    let (win_w, win_h) = (app.surface_size.0 as i32, app.surface_size.1 as i32);
    let mut quads: Quads = Vec::new();

    // The rail strip and one row per tool, active tool in the action colour.
    quads.push((0, 0, RAIL_W, win_h, RAIL_BG));
    for (index, (tool, key)) in Tool::ALL.iter().enumerate() {
        let y = PAD + index as i32 * 32;
        let active = *tool == app.tool;
        quads.push((4, y, RAIL_W - 8, 28, if active { overlay::ACTION } else { TOOL_BG }));
        let label = key.to_string();
        let cell = 2;
        let text_w = text_pixel_width(&label, cell);
        overlay::push_text(
            &mut quads,
            4 + (RAIL_W - 8 - text_w) / 2,
            y + (28 - 5 * cell) / 2,
            &label,
            cell,
            WHITE,
        );
    }

    // Palette swatches, 2 columns × 3 rows; the current ink gets a border.
    let swatch_top = PAD + Tool::ALL.len() as i32 * 32 + 8;
    for (index, colour) in PALETTE.iter().enumerate() {
        let x = 6 + (index as i32 % 2) * 18;
        let y = swatch_top + (index as i32 / 2) * 18;
        if *colour == app.ink.colour {
            quads.push((x - 2, y - 2, 18, 18, WHITE));
        }
        quads.push((x, y, 14, 14, ink_floats(*colour)));
    }

    // The live findings count in the top-right corner.
    let count = app.doc.findings().len().to_string();
    let cell = 2;
    let text_w = text_pixel_width(&count, cell);
    let pad = 4;
    let x = (win_w - text_w - 2 * pad - PAD).max(RAIL_W + PAD);
    quads.push((x - pad, PAD - pad, text_w + 2 * pad, 5 * cell + 2 * pad, COUNT_BG));
    overlay::push_text(&mut quads, x, PAD, &count, cell, WHITE);

    // The text caret while a text edit is in progress.
    if let Some(text) = &app.text {
        let offset = app.capture_offset();
        let caret_x = offset.x + text.origin.x + flatten::text_width(&text.content, TEXT_SIZE) + 1;
        let caret_y = offset.y + text.origin.y;
        quads.push((caret_x, caret_y, 2, 15, WHITE));
    }
    quads
}

/// Pixel width of `text` in the overlay's bitmap font at `cell` size.
fn text_pixel_width(text: &str, cell: i32) -> i32 {
    (text.chars().count() as i32 * (GLYPH_WIDTH as i32 + 1) * cell - cell).max(0)
}

/// A sampled-texture bind-group-layout entry.
fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// A sampler bind-group-layout entry.
fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
        count: None,
    }
}

/// A uniform-buffer bind-group-layout entry.
fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Capture-quad shader: a textured quad placed at `scene.dst` (surface
/// pixels), so the capture draws unscaled and centred right of the rail.
const CAPTURE_WGSL: &str = r"
struct Scene {
    dst: vec4<f32>,
    viewport: vec2<f32>,
};
@group(0) @binding(0) var capture_tex: texture_2d<f32>;
@group(0) @binding(1) var capture_sampler: sampler;
@group(0) @binding(2) var<uniform> scene: Scene;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let corner = corners[index];
    let px = scene.dst.xy + corner * scene.dst.zw;
    let ndc = vec2<f32>(
        px.x / scene.viewport.x * 2.0 - 1.0,
        1.0 - px.y / scene.viewport.y * 2.0,
    );
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = corner;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(capture_tex, capture_sampler, in.uv);
}
";
