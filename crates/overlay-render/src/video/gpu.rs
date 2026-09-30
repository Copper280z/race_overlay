use super::*;
use wgpu::util::DeviceExt;
struct Buffers {
    source_size: (u32, u32),
    output_size: (u32, u32),
    front: wgpu::Buffer,
    rear: wgpu::Buffer,
    output: wgpu::Buffer,
    readback: wgpu::Buffer,
    bind: wgpu::BindGroup,
    last_time: Option<f64>,
}
pub struct GpuProjector {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    uniform: wgpu::Buffer,
    buffers: Option<Buffers>,
}
impl GpuProjector {
    pub fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .map_err(|e| e.to_string())?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("Video projection"),
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Dual fisheye projection"),
            source: wgpu::ShaderSource::Wgsl(include_str!("project.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Video projection"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &[0; 17 * 16],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        Ok(Self {
            device,
            queue,
            pipeline,
            uniform,
            buffers: None,
        })
    }
    pub fn render(
        &mut self,
        images: &LensImages<'_>,
        cal: &DualLensCalibration,
        config: &VideoProcessingConfig,
        correction: Quaternion,
        size: (u32, u32),
    ) -> Result<Vec<u8>, String> {
        validate(images, config, size)?;
        if config.seam == "adaptive" {
            return Err("Advanced stitching requires a registered backend".into());
        }
        let bytes = u64::from(size.0) * u64::from(size.1) * 4;
        let source_bytes = u64::from(images.width) * u64::from(images.height) * 4;
        if bytes * 2 > self.device.limits().max_storage_buffer_binding_size
            || source_bytes > self.device.limits().max_storage_buffer_binding_size
        {
            return Err("Video frame exceeds GPU buffer limits".into());
        }
        if self
            .buffers
            .as_ref()
            .is_none_or(|b| b.source_size != (images.width, images.height) || b.output_size != size)
        {
            let make = |n, usage| {
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: n,
                    usage,
                    mapped_at_creation: false,
                })
            };
            let front = make(
                source_bytes,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            let rear = make(
                source_bytes,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            let output = make(
                bytes,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            );
            let readback = make(
                bytes,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            );
            // Cache the output pixel rays in a unit horizontal field of view.
            // FOV and view rotation remain cheap shader uniforms during dragging.
            let rays = (0..size.1)
                .flat_map(|y| {
                    (0..size.0).map(move |x| {
                        [
                            (x as f32 + 0.5 - size.0 as f32 * 0.5) * 2.0 / size.0 as f32,
                            (y as f32 + 0.5 - size.1 as f32 * 0.5) * 2.0 / size.0 as f32,
                        ]
                    })
                })
                .collect::<Vec<_>>();
            let rays = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Cached output rays"),
                    contents: bytemuck::cast_slice(&rays),
                    usage: wgpu::BufferUsages::STORAGE,
                });
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: front.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: rear.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: output.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: rays.as_entire_binding(),
                    },
                ],
            });
            self.buffers = Some(Buffers {
                source_size: (images.width, images.height),
                output_size: size,
                front,
                rear,
                output,
                readback,
                bind,
                last_time: None,
            });
        }
        let b = self.buffers.as_mut().expect("initialized buffers");
        if b.last_time != Some(images.timestamp) {
            self.queue.write_buffer(&b.front, 0, images.pixels[0]);
            self.queue.write_buffer(&b.rear, 0, images.pixels[1]);
            b.last_time = Some(images.timestamp);
        }
        let mut data = [[0_f32; 4]; 17];
        data[0] = [
            size.0 as f32,
            size.1 as f32,
            (config.horizontal_fov_degrees.to_radians() * 0.5).tan() as f32,
            if config.seam == "feather" {
                (2. * config.feather_degrees.to_radians()) as f32
            } else {
                0.
            },
        ];
        for (i, row) in correction
            .multiply(config.view.rotation())
            .matrix()
            .iter()
            .enumerate()
        {
            data[1 + i][..3].copy_from_slice(&row.map(|v| v as f32));
        }
        for (i, l) in cal.lenses.iter().enumerate() {
            let base = 4 + i * 6;
            for (j, row) in l.rig_to_lens.matrix().iter().enumerate() {
                data[base + j][..3].copy_from_slice(&row.map(|v| v as f32));
            }
            data[base + 3] = [
                l.focal[0] as f32,
                l.focal[1] as f32,
                l.center[0] as f32,
                l.center[1] as f32,
            ];
            data[base + 4] = [
                l.distortion[0] as f32,
                l.distortion[1] as f32,
                l.distortion[2] as f32,
                l.xi as f32,
            ];
            data[base + 5] = [
                l.distortion[3] as f32,
                l.distortion[4] as f32,
                l.max_angle as f32,
                0.,
            ];
        }
        data[16] = [images.width as f32, images.height as f32, 0., 0.];
        self.queue
            .write_buffer(&self.uniform, 0, bytemuck::cast_slice(&data));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &b.bind, &[]);
            pass.dispatch_workgroups(size.0.div_ceil(16), size.1.div_ceil(16), 1);
        }
        encoder.copy_buffer_to_buffer(&b.output, 0, &b.readback, 0, bytes);
        let submission = self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        b.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .map_err(|e| e.to_string())?;
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let result = b
            .readback
            .slice(..)
            .get_mapped_range()
            .map_err(|e| e.to_string())?
            .to_vec();
        b.readback.unmap();
        Ok(result)
    }
}
