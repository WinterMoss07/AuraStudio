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
    pub canvas:      Canvas,
    window:          Arc<Window>,
    egui_ctx:        egui::Context,
    egui_state:      egui_winit::State,
    egui_renderer:   egui_wgpu::Renderer,
    viewport:        [f32; 4],
}

impl GpuState {
    pub async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();

        let backend = if cfg!(target_os = "windows") {
            Backends::VULKAN
        } else if cfg!(target_os = "macos") {
            Backends::METAL
        } else {
            Backends::VULKAN
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
                    required_limits:   Limits::default(),
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

        // Latency: Mailbox shows the newest frame at the next vsync without
        // tearing and without queueing old frames. Fifo (classic vsync, the
        // fallback) can queue frames, each adding ~16 ms of lag at 60 Hz.
        // Rendering only happens on demand (ControlFlow::Wait), so Mailbox
        // doesn't make the GPU spin.
        let present_mode = if caps.present_modes.contains(&PresentMode::Mailbox) {
            PresentMode::Mailbox
        } else {
            PresentMode::Fifo
        };
        log::info!("surface: {format:?}, present mode {present_mode:?} (available: {:?})", caps.present_modes);

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
            canvas_size: [2048.0, 2048.0],
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

        let canvas = Canvas::new(&device, &queue, 2048, 2048);

        let view_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label:   Some("view_bind_group"),
            layout:  &canvas_bgl,
            entries: &[
                BindGroupEntry {
                    binding:  0,
                    resource: BindingResource::TextureView(&canvas.view),
                },
                BindGroupEntry {
                    binding:  1,
                    resource: BindingResource::Sampler(&canvas.sampler),
                },
                BindGroupEntry {
                    binding:  2,
                    resource: view_buffer.as_entire_binding(),
                },
            ],
        });

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
            canvas_pipeline, view_buffer, view_bind_group,
            canvas, window,
            egui_ctx, egui_state, egui_renderer,
            viewport: [0.0, 0.0, size.width.max(1) as f32, size.height.max(1) as f32],
        }
    }

    /// Transmet un WindowEvent à egui. Retourne true si egui a consommé
    /// l'event (clic sur un widget, etc.) — dans ce cas l'app ne doit
    /// pas aussi interpréter l'event comme un coup de pinceau.
    pub fn egui_handle_event(&mut self, event: &winit::event::WindowEvent) -> bool {
        self.egui_state.on_window_event(&self.window, event).consumed
    }

    /// Zone de la fenêtre (en pixels physiques) où le canvas est dessiné :
    /// tout l'espace non couvert par les docks egui. (x, y, largeur, hauteur)
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
        // Recalculé précisément à la prochaine frame depuis la mise en
        // page egui ; en attendant, toute la fenêtre.
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

        // Paquet contigu : on extrait les lignes de la zone dirty uniquement.
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

    /// Dessine une frame. egui tourne en premier pour connaître la place
    /// prise par les docks ; le canvas est ensuite dessiné uniquement dans
    /// la zone restante, puis l'UI par-dessus.
    pub fn render(&mut self, ui_fn: impl FnOnce(&egui::Context)) {
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let mut ui_fn = Some(ui_fn);
        let mut free_rect = None;
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            if let Some(f) = ui_fn.take() { f(ctx); }
            free_rect = Some(ctx.available_rect());
        });
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
