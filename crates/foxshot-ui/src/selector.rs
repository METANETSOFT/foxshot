//! The interactive region selector: a borderless, always-on-top window
//! over the captured frame, rendered with wgpu, driven by winit events.
//! All selection decisions live in [`foxshot_core::SelectionState`].

use crate::overlay::{self, OverlayVertex};
use foxshot_core::{Frame, Point, Rect, SelectionState};
use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowLevel};

/// Anything that can go wrong bringing the selector up.
#[derive(Debug)]
pub enum UiError {
    /// The event loop or window could not be created.
    Windowing(String),
    /// No GPU adapter/device, or surface setup failed.
    Gpu(String),
}

impl fmt::Display for UiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UiError::Windowing(message) => write!(f, "windowing: {message}"),
            UiError::Gpu(message) => write!(f, "gpu: {message}"),
        }
    }
}

impl std::error::Error for UiError {}

/// The interactive region selector.
///
/// [`RegionSelector::run`] opens a borderless, always-on-top window sized
/// to `bounds`, shows `frame`, and lets the user drag out a region. The
/// capture is drawn at full brightness inside the selection and dimmed
/// outside it, with a 1px border in the action colour, eight handles and
/// a live `WIDTH x HEIGHT` readout.
///
/// Input: left-press begins, motion drags, release finishes, Escape
/// cancels, Shift holds the square lock, arrows nudge by 1 px (Shift+arrow
/// by 10), Enter confirms. Returns the selected rectangle in frame
/// coordinates, or `None` when the user cancelled.
#[derive(Debug)]
pub struct RegionSelector;

impl RegionSelector {
    /// Runs the selector modally and returns the chosen region (or `None`
    /// on cancel).
    pub fn run(frame: Frame, bounds: Rect) -> Result<Option<Rect>, UiError> {
        let event_loop =
            EventLoop::new().map_err(|e| UiError::Windowing(format!("event loop: {e}")))?;
        event_loop.set_control_flow(ControlFlow::Wait);
        let mut app = SelectorApp::new(frame, bounds);
        event_loop
            .run_app(&mut app)
            .map_err(|e| UiError::Windowing(format!("run: {e}")))?;
        Ok(app.result)
    }
}

/// The winit application: window plus GPU state plus the core state machine.
struct SelectorApp {
    frame: Frame,
    bounds: Rect,
    selection: SelectionState,
    result: Option<Rect>,
    gpu: Option<Gpu>,
}

impl SelectorApp {
    fn new(frame: Frame, bounds: Rect) -> Self {
        // Selection coordinates are frame coordinates: the window covers
        // exactly the frame, origin top-left.
        let selection =
            SelectionState::new(Rect::from_xywh(0, 0, bounds.size.width, bounds.size.height));
        Self { frame, bounds, selection, result: None, gpu: None }
    }
}

impl ApplicationHandler for SelectorApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        match Gpu::new(event_loop, &self.frame, self.bounds) {
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
                self.selection.cancel();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let point = Point { x: position.x as i32, y: position.y as i32 };
                self.selection.drag_to(point);
                window.request_redraw();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                let point = self.selection.cursor();
                match state {
                    ElementState::Pressed => self.selection.begin(point),
                    ElementState::Released => self.selection.finish(),
                }
                window.request_redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.selection.set_square_lock(modifiers.state().shift_key());
                window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                let step = if self.selection.square_lock() { 10 } else { 1 };
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        self.selection.cancel();
                        event_loop.exit();
                    }
                    Key::Named(NamedKey::Enter) => {
                        self.result = self.selection.rect().filter(|r| !r.is_empty());
                        event_loop.exit();
                    }
                    Key::Named(NamedKey::ArrowLeft) => self.selection.nudge(-step, 0),
                    Key::Named(NamedKey::ArrowRight) => self.selection.nudge(step, 0),
                    Key::Named(NamedKey::ArrowUp) => self.selection.nudge(0, -step),
                    Key::Named(NamedKey::ArrowDown) => self.selection.nudge(0, step),
                    _ => {}
                }
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.render(&self.selection);
                }
            }
            _ => {}
        }
    }
}

/// Scene uniform for the capture shader: selection rect in pixels, the
/// texture size, whether a selection exists, and the dim factor.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SceneUniform {
    sel: [f32; 4],
    size: [f32; 2],
    has_sel: f32,
    dim: f32,
}

/// Overlay uniform: the surface size, for pixel-to-clip conversion.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct OverlayUniform {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

/// Everything wgpu needs to draw one window.
struct Gpu {
    window: Arc<Window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    frame_width: u32,
    frame_height: u32,
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
    fn new(event_loop: &ActiveEventLoop, frame: &Frame, bounds: Rect) -> Result<Self, UiError> {
        let attributes = Window::default_attributes()
            .with_title("FoxShot — select a region")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(winit::dpi::PhysicalPosition::new(bounds.left(), bounds.top()))
            .with_inner_size(winit::dpi::PhysicalSize::new(
                bounds.size.width.max(1),
                bounds.size.height.max(1),
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

        let size = window.inner_size();
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
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        // The captured frame as an RGBA8 texture.
        let frame_size = frame.size();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture"),
            size: wgpu::Extent3d {
                width: frame_size.width,
                height: frame_size.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            frame.bytes(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(frame_size.width * 4),
                rows_per_image: Some(frame_size.height),
            },
            wgpu::Extent3d {
                width: frame_size.width,
                height: frame_size.height,
                depth_or_array_layers: 1,
            },
        );
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
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
                uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
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
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(OVERLAY_WGSL)),
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
            frame_width: frame_size.width,
            frame_height: frame_size.height,
            capture_pipeline,
            capture_bind_group,
            scene_uniform,
            overlay_pipeline,
            overlay_bind_group,
            overlay_uniform,
            overlay_vertices,
        };
        gpu.upload_scene_uniform(&SceneUniform {
            sel: [0.0; 4],
            size: [frame_size.width as f32, frame_size.height as f32],
            has_sel: 0.0,
            dim: 0.35,
        });
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

    fn upload_scene_uniform(&self, uniform: &SceneUniform) {
        self.queue.write_buffer(&self.scene_uniform, 0, bytemuck::bytes_of(uniform));
    }

    /// Draws one frame: the capture (bright inside the selection, dimmed
    /// outside) plus the overlay quads.
    fn render(&mut self, selection: &SelectionState) {
        let (sel, has_sel) = match selection.rect().filter(|r| !r.is_empty()) {
            Some(rect) => (
                [
                    rect.left() as f32,
                    rect.top() as f32,
                    rect.right() as f32,
                    rect.bottom() as f32,
                ],
                1.0,
            ),
            None => ([0.0; 4], 0.0),
        };
        self.upload_scene_uniform(&SceneUniform {
            sel,
            size: [self.frame_width as f32, self.frame_height as f32],
            has_sel,
            dim: 0.35,
        });

        let vertices = overlay::build(selection);
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
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("selector") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("selector pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
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
            pass.draw(0..3, 0..1);
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

/// Fullscreen-triangle capture shader: samples the frame, dims every
/// pixel outside the selection.
const CAPTURE_WGSL: &str = r"
struct Scene {
    sel: vec4<f32>,
    size: vec2<f32>,
    has_sel: f32,
    dim: f32,
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
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, -1.0),
        vec2<f32>(0.0, -1.0),
    );
    var out: VsOut;
    out.clip = vec4<f32>(positions[index], 0.0, 1.0);
    out.uv = uvs[index];
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var colour = textureSample(capture_tex, capture_sampler, in.uv);
    let px = vec2<f32>(in.uv.x * scene.size.x, (1.0 - in.uv.y) * scene.size.y);
    if scene.has_sel > 0.5
        && (px.x < scene.sel.x || px.y < scene.sel.y
            || px.x >= scene.sel.z || px.y >= scene.sel.w) {
        colour = vec4<f32>(colour.rgb * scene.dim, colour.a);
    }
    return colour;
}
";

/// Coloured-quad overlay shader for the border, handles and readout.
const OVERLAY_WGSL: &str = r"
struct Viewport {
    size: vec2<f32>,
};
@group(0) @binding(0) var<uniform> viewport: Viewport;

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let ndc = vec2<f32>(
        in.pos.x / viewport.size.x * 2.0 - 1.0,
        1.0 - in.pos.y / viewport.size.y * 2.0,
    );
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
";
