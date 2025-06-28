// This example demonstrates a how to generate audio using cunes via compute shaders
use cuneus::{Core, ShaderApp, ShaderManager, RenderKit, UniformProvider, UniformBinding, ShaderControls};
use cuneus::SynthesisManager;
use cuneus::compute::{ComputeShaderConfig, COMPUTE_TEXTURE_FORMAT_RGBA16};
use winit::event::*;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    
    let (app, event_loop) = ShaderApp::new("Synth", 800, 600);
    app.run(event_loop, |core| {
        SynthManager::init(core)
    })
}

// Parameters passed from CPU to GPU shader
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct SynthParams {
    tempo: f32,
    scale_type: u32,
    visualizer_mode: u32,
    bass_boost: f32,
    melody_octave: f32,
    harmony_mix: f32,
    reverb_amount: f32,
    volume: f32,
}

impl UniformProvider for SynthParams {
    fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

struct SynthManager {
    base: RenderKit,
    params_uniform: UniformBinding<SynthParams>,
    gpu_synthesis: Option<SynthesisManager>,
}

impl SynthManager {
    fn update_synthesis_visualization(&mut self, _queue: &wgpu::Queue) {
        // Parameters automatically sync to GPU via params_uniform
    }
}

impl ShaderManager for SynthManager {
    fn init(core: &Core) -> Self {
        
        let texture_bind_group_layout = core.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Texture Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        
        let params_bind_group_layout = core.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
            label: Some("params_bind_group_layout"),
        });
        
        let mut base = RenderKit::new(
            core,
            include_str!("../../shaders/vertex.wgsl"),
            include_str!("../../shaders/blit.wgsl"),
            &[&texture_bind_group_layout],
            None,
        );
        

        let config = ComputeShaderConfig {
            workgroup_size: [16, 16, 1],
            workgroup_count: Some([64, 4, 1]), // Fixed workgroups: 64*16=1024 samples in X, independent of window size
            dispatch_once: false,
            storage_texture_format: COMPUTE_TEXTURE_FORMAT_RGBA16,
            enable_atomic_buffer: false,
            atomic_buffer_multiples: 4,
            entry_points: vec!["main".to_string()],
            sampler_address_mode: wgpu::AddressMode::ClampToEdge,
            sampler_filter_mode: wgpu::FilterMode::Linear,
            label: "Synth".to_string(),
            mouse_bind_group_layout: Some(params_bind_group_layout.clone()),
            enable_fonts: false,
            enable_audio_buffer: true,
            audio_buffer_size: 1024,
        };
        
        let params_uniform = UniformBinding::new(
            &core.device,
            "Synth Params",
            SynthParams {
                tempo: 120.0,
                scale_type: 0,
                visualizer_mode: 0,
                bass_boost: 1.0,
                melody_octave: 4.0,
                harmony_mix: 0.3,
                reverb_amount: 0.2,
                volume: 0.3,
            },
            &params_bind_group_layout,
            0,
        );

        base.compute_shader = Some(cuneus::compute::ComputeShader::new_with_config(
            core,
            include_str!("../../shaders/synth.wgsl"),
            config,
        ));
        
        if let Some(compute_shader) = &mut base.compute_shader {
            compute_shader.add_mouse_uniform_binding(&params_uniform.bind_group, 2);
        }
        
        if let Some(compute_shader) = &mut base.compute_shader {
            let shader_module = core.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Synth Compute Shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/synth.wgsl").into()),
            });
            if let Err(_e) = compute_shader.enable_hot_reload(
                core.device.clone(),
                PathBuf::from("shaders/synth.wgsl"),
                shader_module,
            ) {
            }
        }
        
        let gpu_synthesis = match SynthesisManager::new() {
            Ok(mut synth) => {
                if let Err(_e) = synth.start_gpu_synthesis() {
                    None
                } else {
                    Some(synth)
                }
            },
            Err(_e) => {
                None
            }
        };
        
        
        Self {
            base,
            params_uniform,
            gpu_synthesis,
        }
    }
    
    fn update(&mut self, core: &Core) {
        self.base.fps_tracker.update();
        
        let current_time = self.base.controls.get_time(&self.base.start_time);
        let delta = 1.0 / 60.0;
        self.base.update_compute_shader_time(current_time, delta, &core.queue);
        
        
        // Read GPU-generated audio parameters for CPU synthesis
        if self.base.time_uniform.data.frame % 180 == 0 {
            if let Some(compute_shader) = &self.base.compute_shader {
                if let Ok(gpu_samples) = pollster::block_on(compute_shader.read_audio_samples(&core.device, &core.queue)) {
                    if gpu_samples.len() >= 3 {
                        let frequency = gpu_samples[0];
                        let amplitude = gpu_samples[1];
                        let waveform_type = gpu_samples[2] as u32;
                        
                        if let Some(ref mut synth) = self.gpu_synthesis {
                            synth.update_synth_params(frequency, amplitude, waveform_type);
                        }
                    }
                }
            }
        }
        
        self.update_synthesis_visualization(&core.queue);
        
        if let Some(ref mut synth) = self.gpu_synthesis {
            synth.update();
        }
    }
    
    
    fn render(&mut self, core: &Core) -> Result<(), wgpu::SurfaceError> {
        let output = core.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = core.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Synth Render Encoder"),
        });
        
        let mut params = self.params_uniform.data;
        let mut changed = false;
        let mut controls_request = self.base.controls.get_ui_request(&self.base.start_time, &core.size);
        controls_request.current_fps = Some(self.base.fps_tracker.fps());
        
        let full_output = if self.base.key_handler.show_ui {
            self.base.render_ui(core, |ctx| {
                ctx.style_mut(|style| {
                    style.visuals.window_fill = egui::Color32::from_rgba_premultiplied(0, 0, 0, 180);
                    style.text_styles.get_mut(&egui::TextStyle::Body).unwrap().size = 11.0;
                    style.text_styles.get_mut(&egui::TextStyle::Button).unwrap().size = 10.0;
                });
                
                egui::Window::new("Cuneus Synth")
                    .collapsible(true)
                    .resizable(true)
                    .default_width(250.0)
                    .show(ctx, |ui| {
                        ui.vertical(|ui| {
                            ui.label("GPU Synthesizer");
                            ui.separator();
                            
                            ui.label("Musical Parameters:");
                            changed |= ui.add(egui::Slider::new(&mut params.tempo, 60.0..=180.0).text("Tempo")).changed();
                            changed |= ui.add(egui::Slider::new(&mut params.melody_octave, 3.0..=6.0).text("Octave")).changed();
                            
                            ui.separator();
                            ui.label("🔊 Output:");
                            changed |= ui.add(egui::Slider::new(&mut params.volume, 0.0..=0.5).text("Volume")).changed();
                            
                            ui.separator();
                            ui.label("How it works?");
                            ui.label("• Shader generates frequencies mathematically");
                            ui.label("• GPU writes to buffer, CPU reads for audio");
                            ui.label("• Same data drives visual feedback");
                            ui.label("• Real-time parameter control");
                            
                            ui.separator();
                            ShaderControls::render_controls_widget(ui, &mut controls_request);
                        });
                    });
            })
        } else {
            self.base.render_ui(core, |_ctx| {})
        };
        
        if changed {
            self.params_uniform.data = params;
            self.params_uniform.update(&core.queue);
        }
        
        let current_time = self.base.controls.get_time(&self.base.start_time);
        let delta = 1.0 / 60.0;
        self.base.update_compute_shader_time(current_time, delta, &core.queue);
        
        self.update_synthesis_visualization(&core.queue);
        
        self.base.dispatch_compute_shader(&mut encoder, core);
        
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Display Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            
            if let Some(compute_texture) = self.base.get_compute_output_texture() {
                render_pass.set_pipeline(&self.base.renderer.render_pipeline);
                render_pass.set_vertex_buffer(0, self.base.renderer.vertex_buffer.slice(..));
                render_pass.set_bind_group(0, &compute_texture.bind_group, &[]);
                render_pass.draw(0..4, 0..1);
            }
        }
        
        self.base.apply_control_request(controls_request);
        self.base.handle_render_output(core, &view, full_output, &mut encoder);
        core.queue.submit(Some(encoder.finish()));
        output.present();
        Ok(())
    }
    
    fn resize(&mut self, core: &Core) {
        self.base.update_resolution(&core.queue, core.size);
        self.base.resize_compute_shader(core);
    }
    
    fn handle_input(&mut self, core: &Core, event: &WindowEvent) -> bool {
        if self.base.egui_state.on_window_event(core.window(), event).consumed {
            return true;
        }
        
        if let WindowEvent::KeyboardInput { event, .. } = event {
            return self.base.key_handler.handle_keyboard_input(core.window(), event);
        }
        
        false
    }
}