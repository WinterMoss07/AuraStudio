use std::sync::Arc;
use wgpu::*;
use wgpu::util::DeviceExt;
use winit::window::Window;
use aurastudio_canvas::Canvas;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ViewUniform {
    canvas_size: [f32; 2],
    window_size: [f32; 2],
}

pub struct GpuState {
    surface:         Surface<'static>,
    device:          Device,
    queue:           Queue,
    config:          SurfaceConfiguration,
    canvas_pipeline: RenderPipeline,
    view_buffer:     Buffer,
    view_bind_group: BindGroup,
    canvas_bgl:      BindGroupLayout,
    pub canvas:      Canvas,
    window:          Arc<Window>,
    egui_ctx:        egui::Context,
    egui_state:      egui_winit::State,
    egui_renderer:   egui_wgpu::Renderer,
    viewport:        [f32; 4],
    text_input_active: bool,
}

impl GpuState {
    /// `canvas_size`: initial canvas width and height in pixels.
    pub async fn new(window: Arc<Window>, canvas_size: (u32, u32)) -> Self {
        let size = window.inner_size();

        // Vulkan by default (lowest RAM in our tests), Metal on macOS.
        // AURASTUDIO_BACKEND=dx12|vulkan|metal|gl overrides it, to compare
        // latency per GPU driver without rebuilding.
        let backend = match std::env::var("AURASTUDIO_BACKEND").ok().as_deref() {
            Some("dx12")   => Backends::DX12,
            Some("vulkan") => Backends::VULKAN,
            Some("metal")  => Backends::METAL,
            Some("gl")     => Backends::GL,
            _ if cfg!(target_os = "macos") => Backends::METAL,
            _ => Backends::VULKAN,
        };

        let instance = Instance::new(InstanceDescriptor {
            backends: backend,
            ..Default::default()
        });

        let surface = instance.create_surface(Arc::clone(&window)).unwrap();

        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference:       PowerPreference::HighPerformance,
                compatible_surface:     Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("No GPU adapter found");

        let (device, queue) = adapter
            .request_device(
                &DeviceDescriptor {
                    label:             None,
                    required_features: Features::empty(),
                    // Ask for the biggest texture the GPU supports (often
                    // 16384) instead of wgpu's portable default (8192): it
                    // caps the canvas size.
                    required_limits:   Limits {
                        max_texture_dimension_2d: adapter.limits().max_texture_dimension_2d,
                        ..Limits::default()
                    },
                    memory_hints:      MemoryHints::default(),
                },
                None,
            )
            .await
            .expect("Failed to create device");

        let caps   = surface.get_capabilities(&adapter);
        // Non-sRGB surface: canvas pixels are already sRGB-encoded bytes and
        // must reach the screen unchanged. With an sRGB surface the GPU
        // encoded them a second time: everything looked lighter, mid-tones
        // were compressed and gradients banded. egui also expects this.
        let format = caps.formats.iter().copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);

        // Latency, best first:
        // - Mailbox: newest frame shown at the next vsync, never queued, no
        //   tearing. Not offered by every driver (Intel Vulkan on Windows
        //   only has Immediate + Fifo).
        // - Immediate: presented right away, no wait for vsync. In a normal
        //   window the Windows compositor (DWM) still syncs to the display,
        //   so it doesn't tear there.
        // - Fifo: classic vsync. Rendering blocks until the next refresh, and
        //   pen samples arriving meanwhile wait one more frame: measured as
        //   ~14 ms per frame on an Intel Arc 140T, the main remaining lag.
        // Rendering only happens on demand (ControlFlow::Wait), so neither
        // Mailbox nor Immediate makes the GPU spin when idle.
        // AURASTUDIO_PRESENT=mailbox|immediate|fifo overrides the choice.
        let wanted = match std::env::var("AURASTUDIO_PRESENT").ok().as_deref() {
            Some("mailbox")   => vec![PresentMode::Mailbox],
            Some("immediate") => vec![PresentMode::Immediate],
            Some("fifo")      => vec![PresentMode::Fifo],
            _ => vec![PresentMode::Mailbox, PresentMode::Immediate],
        };
        let present_mode = wanted.into_iter()
            .find(|m| caps.present_modes.contains(m))
            .unwrap_or(PresentMode::Fifo);
        log::info!("surface: {format:?}, present mode {present_mode:?} (available: {:?}), backend {:?}",
            caps.present_modes, adapter.get_info().backend);

        let config = SurfaceConfiguration {
            usage:                         TextureUsages::RENDER_ATTACHMENT,
            format,
            width:                         size.width.max(1),
            height:                        size.height.max(1),
            present_mode,
            alpha_mode:                    caps.alpha_modes[0],
            view_formats:                  vec![],
            // At most 1 frame in flight: a new stroke sample is never stuck
            // behind an older frame waiting to be shown.
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&device, &config);

        let canvas_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label:   Some("canvas_bgl"),
            entries: &[
                BindGroupLayoutEntry {
                    binding:    0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        multisampled:   false,
                        view_dimension: TextureViewDimension::D2,
                        sample_type:    TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding:    1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding:    2,
                    visibility: ShaderStages::VERTEX,
                    ty: BindingType::Buffer {
                        ty:                 BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count: None,
                },
            ],
        });

        let view_uniform = ViewUniform {
            canvas_size: [canvas_size.0 as f32, canvas_size.1 as f32],
            window_size: [size.width as f32, size.height as f32],
        };

        let view_buffer = device.create_buffer_init(&util::BufferInitDescriptor {
            label:    Some("view_buffer"),
            contents: bytemuck::cast_slice(&[view_uniform]),
            usage:    BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });

        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label:  Some("canvas_shader"),
            source: ShaderSource::Wgsl(include_str!("shaders/canvas.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label:                Some("canvas_pipeline_layout"),
            bind_group_layouts:   &[&canvas_bgl],
            push_constant_ranges: &[],
        });

        let canvas_pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label:  Some("canvas_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module:              &shader,
                entry_point:         Some("vs_main"),
                buffers:             &[],
                compilation_options: PipelineCompilationOptions::default(),
            },
            fragment: Some(FragmentState {
                module:              &shader,
                entry_point:         Some("fs_main"),
                targets:             &[Some(ColorTargetState {
                    format,
                    blend:      Some(BlendState::REPLACE),
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: PipelineCompilationOptions::default(),
            }),
            primitive:     PrimitiveState::default(),
            depth_stencil: None,
            multisample:   MultisampleState::default(),
            multiview:     None,
            cache:         None,
        });

        let canvas = Canvas::new(&device, &queue, canvas_size.0, canvas_size.1);
        let view_bind_group = canvas_bind_group(&device, &canvas_bgl, &canvas, &view_buffer);

        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);

        Self {
            surface, device, queue, config,
            canvas_pipeline, view_buffer, view_bind_group, canvas_bgl,
            canvas, window,
            egui_ctx, egui_state, egui_renderer,
            viewport: [0.0, 0.0, size.width.max(1) as f32, size.height.max(1) as f32],
            text_input_active: false,
        }
    }

    /// Biggest canvas side the GPU accepts (max 2D texture size).
    pub fn max_canvas_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// Replace the canvas texture with a new size. The caller then fills
    /// `canvas.pixels` (layer composite) and marks it dirty.
    pub fn resize_canvas(&mut self, width: u32, height: u32) {
        self.canvas = Canvas::new(&self.device, &self.queue, width, height);
        self.view_bind_group = canvas_bind_group(&self.device, &self.canvas_bgl, &self.canvas, &self.view_buffer);
        // Force the view uniform (canvas size) to be rewritten.
        let vp = self.viewport;
        self.viewport = [-1.0; 4];
        self.set_viewport(vp);
    }

    /// True while an egui text/number field is being edited: shortcuts like
    /// Delete must then go to the field, not the canvas.
    ///
    /// Not egui's wants_keyboard_input(): that is true whenever ANY widget
    /// has focus, including a button just clicked (e.g. the delete-layer
    /// button), which silently disabled the Delete shortcut.
    pub fn ui_wants_keyboard(&self) -> bool {
        self.text_input_active
    }

    /// Forward a WindowEvent to egui. Returns egui's "consumed" flag (the
    /// event hit a widget); the app only uses it to request a new frame,
    /// see is_canvas_at for deciding whether a press starts a stroke.
    pub fn egui_handle_event(&mut self, event: &winit::event::WindowEvent) -> bool {
        self.egui_state.on_window_event(&self.window, event).consumed
    }

    /// Window area (physical pixels) where the canvas is drawn: all the
    /// space not covered by egui docks. (x, y, width, height)
    pub fn canvas_viewport(&self) -> [f32; 4] {
        self.viewport
    }

    /// True if a press at `pos` (physical window pixels) should start a
    /// stroke: inside the canvas area and not under any egui panel.
    ///
    /// We don't rely on egui's own "consumed" flag for this: it is based on
    /// the pointer position of egui's *last frame*, which is stale because
    /// we only redraw on demand (ControlFlow::Wait), so it rejected clicks
    /// made on the canvas.
    pub fn is_canvas_at(&self, pos: (f32, f32)) -> bool {
        let [vx, vy, vw, vh] = self.viewport;
        let inside = pos.0 >= vx && pos.0 < vx + vw && pos.1 >= vy && pos.1 < vy + vh;
        let ppp = self.egui_ctx.pixels_per_point();
        // Docked panels are already outside the viewport. Here we only look
        // for floating windows/popups above the canvas: egui keeps a
        // full-window Background-order layer, which must not count.
        let over_ui = self.egui_ctx
            .layer_id_at(egui::pos2(pos.0 / ppp, pos.1 / ppp))
            .is_some_and(|layer| layer.order != egui::Order::Background);
        inside && !over_ui
    }

    pub fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 { return; }
        self.config.width  = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        // Recomputed from the egui layout on the next frame; until then,
        // the whole window.
        self.set_viewport([0.0, 0.0, size.width as f32, size.height as f32]);
    }

    fn set_viewport(&mut self, vp: [f32; 4]) {
        if vp == self.viewport { return; }
        self.viewport = vp;
        let uniform = ViewUniform {
            canvas_size: [self.canvas.width as f32, self.canvas.height as f32],
            window_size: [vp[2], vp[3]],
        };
        self.queue.write_buffer(&self.view_buffer, 0, bytemuck::cast_slice(&[uniform]));
    }

    pub fn upload_canvas(&mut self) {
        let Some((x0, y0, x1, y1)) = self.canvas.dirty_rect else { return; };
        let w = self.canvas.width;
        let rw = x1 - x0;
        let rh = y1 - y0;

        // Contiguous upload buffer: only the rows of the dirty rect.
        let mut buf = Vec::with_capacity((rw * rh * 4) as usize);
        for y in y0..y1 {
            let row_start = ((y * w + x0) * 4) as usize;
            let row_end   = row_start + (rw * 4) as usize;
            buf.extend_from_slice(&self.canvas.pixels[row_start..row_end]);
        }

        self.queue.write_texture(
            ImageCopyTexture {
                texture:   &self.canvas.texture,
                mip_level: 0,
                origin:    Origin3d { x: x0, y: y0, z: 0 },
                aspect:    TextureAspect::All,
            },
            &buf,
            ImageDataLayout {
                offset:         0,
                bytes_per_row:  Some(rw * 4),
                rows_per_image: Some(rh),
            },
            Extent3d { width: rw, height: rh, depth_or_array_layers: 1 },
        );
    }

    /// Draw a frame. egui runs first to know how much room the docks take;
    /// the canvas is then drawn only in the remaining area, and the UI on
    /// top of it.
    pub fn render(&mut self, ui_fn: impl FnOnce(&egui::Context)) {
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let mut ui_fn = Some(ui_fn);
        let mut free_rect = None;
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            if let Some(f) = ui_fn.take() { f(ctx); }
            free_rect = Some(ctx.available_rect());
        });
        // egui asks for another frame right away when its layout isn't done
        // (e.g. windows sizing themselves on the first frames). With
        // ControlFlow::Wait nothing else would trigger it: panels stayed
        // invisible at startup until the mouse moved.
        if full_output.viewport_output.get(&egui::ViewportId::ROOT)
            .is_some_and(|v| v.repaint_delay.is_zero())
        {
            self.window.request_redraw();
        }

        // egui asks for IME (text input) only while a text field is focused.
        self.text_input_active = full_output.platform_output.ime.is_some();
        self.egui_state.handle_platform_output(&self.window, full_output.platform_output);

        let ppp = full_output.pixels_per_point;
        if let Some(r) = free_rect {
            let (ww, wh) = (self.config.width as f32, self.config.height as f32);
            let x0 = (r.min.x * ppp).round().clamp(0.0, ww);
            let y0 = (r.min.y * ppp).round().clamp(0.0, wh);
            let x1 = (r.max.x * ppp).round().clamp(x0, ww);
            let y1 = (r.max.y * ppp).round().clamp(y0, wh);
            self.set_viewport([x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0)]);
        }

        if self.canvas.dirty_rect.is_some() {
            self.upload_canvas();
            self.canvas.dirty_rect = None;
        }

        let output = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(SurfaceError::Lost | SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(e) => { log::error!("Surface error: {e:?}"); return; }
        };

        let view = output.texture.create_view(&TextureViewDescriptor::default());
        let mut enc = self.device.create_command_encoder(
            &CommandEncoderDescriptor { label: Some("frame") }
        );

        {
            let mut pass = enc.begin_render_pass(&RenderPassDescriptor {
                label: Some("canvas_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view:           &view,
                    resolve_target: None,
                    ops: Operations {
                        // Gray around the canvas. Same look as before the
                        // non-sRGB surface switch (0.13 linear ≈ 0.40 sRGB).
                        load:  LoadOp::Clear(Color { r: 0.40, g: 0.40, b: 0.40, a: 1.0 }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes:         None,
                occlusion_query_set:      None,
            });

            let [vx, vy, vw, vh] = self.viewport;
            pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
            pass.set_pipeline(&self.canvas_pipeline);
            pass.set_bind_group(0, &self.view_bind_group, &[]);
            pass.draw(0..6, 0..1);
        }

        // --- Passe egui (docks) par-dessus le canvas ---
        let tris = self.egui_ctx.tessellate(full_output.shapes, ppp);
        for (id, delta) in &full_output.textures_delta.set {
            self.egui_renderer.update_texture(&self.device, &self.queue, *id, delta);
        }

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels:    [self.config.width, self.config.height],
            pixels_per_point:  ppp,
        };
        self.egui_renderer.update_buffers(&self.device, &self.queue, &mut enc, &tris, &screen_descriptor);

        {
            let pass = enc.begin_render_pass(&RenderPassDescriptor {
                label: Some("egui_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view:           &view,
                    resolve_target: None,
                    ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes:         None,
                occlusion_query_set:      None,
            });
            self.egui_renderer.render(&mut pass.forget_lifetime(), &tris, &screen_descriptor);
        }

        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }

        self.queue.submit(std::iter::once(enc.finish()));
        output.present();
    }
}

fn canvas_bind_group(device: &Device, layout: &BindGroupLayout, canvas: &Canvas, view_buffer: &Buffer) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label:   Some("view_bind_group"),
        layout,
        entries: &[
            BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&canvas.view) },
            BindGroupEntry { binding: 1, resource: BindingResource::Sampler(&canvas.sampler) },
            BindGroupEntry { binding: 2, resource: view_buffer.as_entire_binding() },
        ],
    })
}
