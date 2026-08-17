//! OpenGL renderer (via `glow`).
//!
//! Builds a render scene from a loaded [`DataModel`], frames an orbit camera
//! around it, and draws every `BasePart` as a flat-shaded colored box. The
//! window/input handling lives in the examples; this module only touches GL.

use glam::{Mat3, Mat4, Vec3};
use glow::HasContext;

use crate::instance::DataModel;
use crate::value::Value;

/// One box to draw.
#[derive(Debug, Clone)]
pub struct RenderPart {
    pub name: String,
    /// Local-to-world translation (CFrame position).
    pub position: [f32; 3],
    /// Row-major 3x3 rotation (CFrame rotation).
    pub rotation: [f32; 9],
    /// Part `Size` in studs.
    pub size: [f32; 3],
    /// Linear sRGB-ish base color.
    pub color: [f32; 3],
}

/// The immutable data the renderer draws.
#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub parts: Vec<RenderPart>,
    /// World-space bounding box of all parts (`(min, max)`), if any.
    pub bounds: Option<([f32; 3], [f32; 3])>,
}

impl Scene {
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }
}

/// Default part color and size used when properties are missing.
const DEFAULT_COLOR: [f32; 3] = [0.639, 0.635, 0.647];
const DEFAULT_SIZE: [f32; 3] = [4.0, 4.0, 4.0];

/// Collects every `BasePart` in the place into a render scene.
///
/// `Terrain` is skipped (it is a `BasePart` but rendered from voxel data, not
/// as a box).
pub fn build_scene(dm: &DataModel) -> Scene {
    let mut parts = Vec::new();
    for inst in &dm.instances {
        if inst.class == "Terrain" || !inst.is_a("BasePart") {
            continue;
        }

        let size = match inst.get_property("Size") {
            Some(Value::Vector3 { x, y, z }) => [*x, *y, *z],
            _ => DEFAULT_SIZE,
        };

        let (position, rotation) = match inst.get_property("CFrame") {
            Some(Value::CFrame { position, rotation }) => (
                [position[0] as f32, position[1] as f32, position[2] as f32],
                [
                    rotation[0] as f32, rotation[1] as f32, rotation[2] as f32,
                    rotation[3] as f32, rotation[4] as f32, rotation[5] as f32,
                    rotation[6] as f32, rotation[7] as f32, rotation[8] as f32,
                ],
            ),
            _ => ([0.0; 3], [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
        };

        let color = match inst.get_property("Color") {
            Some(Value::Color3 { r, g, b }) => [*r, *g, *b],
            Some(Value::Color3uint8 { r, g, b }) => [*r as f32 / 255.0, *g as f32 / 255.0, *b as f32 / 255.0],
            _ => DEFAULT_COLOR,
        };

        parts.push(RenderPart {
            name: inst.name.clone(),
            position,
            rotation,
            size,
            color,
        });
    }

    let bounds = compute_bounds(&parts);
    Scene { parts, bounds }
}

/// Axis-aligned bounds covering every part corner.
fn compute_bounds(parts: &[RenderPart]) -> Option<([f32; 3], [f32; 3])> {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut any = false;
    for p in parts {
        let rot = Mat3::from_cols_array_2d(&[
            [p.rotation[0], p.rotation[1], p.rotation[2]],
            [p.rotation[3], p.rotation[4], p.rotation[5]],
            [p.rotation[6], p.rotation[7], p.rotation[8]],
        ]);
        for sx in [-1.0, 1.0] {
            for sy in [-1.0, 1.0] {
                for sz in [-1.0, 1.0] {
                    let corner = Vec3::new(p.position[0], p.position[1], p.position[2])
                        + rot * (Vec3::new(p.size[0], p.size[1], p.size[2]) * 0.5 * Vec3::new(sx, sy, sz));
                    min = min.min(corner);
                    max = max.max(corner);
                }
            }
        }
        any = true;
    }
    if !any {
        return None;
    }
    Some(([min.x, min.y, min.z], [max.x, max.y, max.z]))
}

/// Sky color drawn as the clear color.
pub fn sky_color() -> [f32; 3] {
    [0.45, 0.72, 1.0]
}

/// Free-orbit camera: rotates around a target point.
#[derive(Debug, Clone)]
pub struct OrbitCamera {
    pub center: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl OrbitCamera {
    pub fn new() -> Self {
        OrbitCamera {
            center: Vec3::ZERO,
            yaw: -0.7,
            pitch: -0.45,
            // Close enough for a third-person view of an avatar.
            distance: 14.0,
        }
    }

    /// Frames the camera around a scene's bounds.
    pub fn frame_scene(&mut self, scene: &Scene, fov_y: f32) {
        if let Some((min, max)) = scene.bounds {
            self.center = Vec3::new(
                (min[0] + max[0]) * 0.5,
                (min[1] + max[1]) * 0.5,
                (min[2] + max[2]) * 0.5,
            );
            let half = Vec3::new(max[0] - min[0], max[1] - min[1], max[2] - min[2]) * 0.5;
            let diag = (half * 2.0).length();
            let needed = (diag * 0.5 + 2.0) / (fov_y * 0.5).tan();
            self.distance = needed.max(4.0);
        }
    }

    /// Direction from the target to the camera.
    fn offset(&self) -> Vec3 {
        let cp = self.pitch.cos();
        Vec3::new(cp * self.yaw.cos(), self.pitch.sin(), cp * self.yaw.sin())
    }

    pub fn eye(&self) -> Vec3 {
        self.center + self.offset() * self.distance
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.center, Vec3::Y)
    }

    /// Left-drag: orbit the target.
    pub fn drag(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.005;
        self.pitch -= dy * 0.005;
        self.pitch = self.pitch.clamp(-1.55, 1.55);
    }

    /// Scroll: zoom in/out (exponential, ~5% per wheel step).
    pub fn zoom(&mut self, dy: f32) {
        self.distance *= (1.0 - dy * 0.05).clamp(0.7, 1.3);
        self.distance = self.distance.clamp(3.0, 300.0);
    }

    /// WASD: pan the target in the camera plane.
    pub fn pan(&mut self, right: f32, forward: f32, up: f32) {
        let cp = self.pitch.cos();
        let cam_forward = Vec3::new(cp * self.yaw.cos(), self.pitch.sin(), cp * self.yaw.sin());
        let cam_right = cam_forward.cross(Vec3::Y).normalize();
        let cam_up = cam_right.cross(cam_forward).normalize();
        let speed = self.distance * 0.0015;
        self.center += cam_right * right * speed + cam_up * up * speed - cam_forward * forward * speed;
    }

    pub fn projection(&self, aspect: f32, fov_y: f32) -> Mat4 {
        Mat4::perspective_rh_gl(fov_y, aspect, 0.1, 100000.0)
    }
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self::new()
    }
}

/// Draws a [`Scene`] with a single flat-shaded box shader.
pub struct GLRenderer {
    program: glow::Program,
    vao: glow::VertexArray,
    index_count: i32,
    u_mvp: Option<glow::UniformLocation>,
    u_normal_mat: Option<glow::UniformLocation>,
    u_color: Option<glow::UniformLocation>,
}

const VERTEX_SRC: &str = r#"#version 330 core
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec3 a_normal;
uniform mat4 u_mvp;
uniform mat3 u_normal_mat;
out vec3 v_normal;
void main() {
    v_normal = u_normal_mat * a_normal;
    gl_Position = u_mvp * vec4(a_pos, 1.0);
}
"#;

const FRAGMENT_SRC: &str = r#"#version 330 core
in vec3 v_normal;
uniform vec4 u_color;
uniform vec3 u_light_dir;
out vec4 frag_color;
void main() {
    vec3 n = normalize(v_normal);
    float diff = max(dot(n, normalize(u_light_dir)), 0.0);
    float intensity = 0.15 + diff * 0.85;
    frag_color = vec4(u_color.rgb * intensity, u_color.a);
}
"#;

impl GLRenderer {
    /// Creates the shader program and uploads a unit cube.
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        let program = link_program(gl, VERTEX_SRC, FRAGMENT_SRC)?;

        let vao = unsafe { gl.create_vertex_array() }.map_err(|e| e.to_string())?;
        let vbo = unsafe { gl.create_buffer() }.map_err(|e| e.to_string())?;
        let ebo = unsafe { gl.create_buffer() }.map_err(|e| e.to_string())?;

        let (verts, indices) = unit_cube();
        unsafe {
            gl.bind_vertex_array(Some(vao));

            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&verts), glow::STATIC_DRAW);

            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, as_bytes(&indices), glow::STATIC_DRAW);

            // a_pos: 3 floats at offset 0, a_normal: 3 floats at offset 12.
            let stride = 24;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 12);
        }

        unsafe {
            gl.use_program(Some(program));
            let light = Vec3::new(0.4, 0.8, 0.6).normalize();
            gl.uniform_3_f32(gl.get_uniform_location(program, "u_light_dir").as_ref(), light.x, light.y, light.z);
            gl.enable(glow::DEPTH_TEST);
        }

        Ok(GLRenderer {
            program,
            vao,
            index_count: indices.len() as i32,
            u_mvp: unsafe { gl.get_uniform_location(program, "u_mvp") },
            u_normal_mat: unsafe { gl.get_uniform_location(program, "u_normal_mat") },
            u_color: unsafe { gl.get_uniform_location(program, "u_color") },
        })
    }

    /// Clears to the sky color and draws every part.
    pub fn render(&self, gl: &glow::Context, camera: &OrbitCamera, width: u32, height: u32, parts: &[RenderPart]) {
        let (sky_r, sky_g, sky_b) = (sky_color()[0], sky_color()[1], sky_color()[2]);
        let aspect = width as f32 / height.max(1) as f32;
        let proj = camera.projection(aspect, 1.05); // ~60 deg vertical FOV
        let view = camera.view();

        unsafe {
            gl.viewport(0, 0, width as i32, height as i32);
            gl.clear_color(sky_r, sky_g, sky_b, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);

            gl.use_program(Some(self.program));
            gl.bind_vertex_array(Some(self.vao));
        }

        for part in parts {
            self.draw_part(gl, &view, &proj, part);
        }
    }

    fn draw_part(&self, gl: &glow::Context, view: &Mat4, proj: &Mat4, part: &RenderPart) {
        let rot = Mat3::from_cols_array_2d(&[
            [part.rotation[0], part.rotation[1], part.rotation[2]],
            [part.rotation[3], part.rotation[4], part.rotation[5]],
            [part.rotation[6], part.rotation[7], part.rotation[8]],
        ]);
        let model = Mat4::from_scale_rotation_translation(Vec3::from(part.size), glam::Quat::from_mat3(&rot), Vec3::from(part.position));
        let mvp = *proj * *view * model;
        let normal_mat = Mat3::from_mat4(model.inverse().transpose());

        unsafe {
            gl.uniform_matrix_4_f32_slice(self.u_mvp.as_ref(), false, &mvp.to_cols_array());
            gl.uniform_matrix_3_f32_slice(self.u_normal_mat.as_ref(), false, &normal_mat.to_cols_array());
            gl.uniform_4_f32(
                self.u_color.as_ref(),
                part.color[0],
                part.color[1],
                part.color[2],
                1.0,
            );
            gl.draw_elements(glow::TRIANGLES, self.index_count, glow::UNSIGNED_INT, 0);
        }
    }
}

fn link_program(gl: &glow::Context, vs_src: &str, fs_src: &str) -> Result<glow::Program, String> {
    unsafe {
        let vs = gl.create_shader(glow::VERTEX_SHADER).map_err(|e| e.to_string())?;
        gl.shader_source(vs, vs_src);
        gl.compile_shader(vs);
        if !gl.get_shader_compile_status(vs) {
            let msg = gl.get_shader_info_log(vs);
            gl.delete_shader(vs);
            return Err(format!("vertex shader: {msg}"));
        }

        let fs = gl.create_shader(glow::FRAGMENT_SHADER).map_err(|e| e.to_string())?;
        gl.shader_source(fs, fs_src);
        gl.compile_shader(fs);
        if !gl.get_shader_compile_status(fs) {
            let msg = gl.get_shader_info_log(fs);
            gl.delete_shader(vs);
            gl.delete_shader(fs);
            return Err(format!("fragment shader: {msg}"));
        }

        let program = gl.create_program().map_err(|e| e.to_string())?;
        gl.attach_shader(program, vs);
        gl.attach_shader(program, fs);
        gl.link_program(program);
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        if !gl.get_program_link_status(program) {
            let msg = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(format!("program link: {msg}"));
        }
        Ok(program)
    }
}

/// Builds a unit cube ([-0.5, 0.5]^3) as interleaved pos/normal vertices.
fn unit_cube() -> (Vec<f32>, Vec<u32>) {
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        ([1.0, 0.0, 0.0], [[0.5, -0.5, -0.5], [0.5, 0.5, -0.5], [0.5, 0.5, 0.5], [0.5, -0.5, 0.5]]),
        ([-1.0, 0.0, 0.0], [[-0.5, -0.5, 0.5], [-0.5, 0.5, 0.5], [-0.5, 0.5, -0.5], [-0.5, -0.5, -0.5]]),
        ([0.0, 1.0, 0.0], [[-0.5, 0.5, -0.5], [-0.5, 0.5, 0.5], [0.5, 0.5, 0.5], [0.5, 0.5, -0.5]]),
        ([0.0, -1.0, 0.0], [[-0.5, -0.5, 0.5], [-0.5, -0.5, -0.5], [0.5, -0.5, -0.5], [0.5, -0.5, 0.5]]),
        ([0.0, 0.0, 1.0], [[-0.5, -0.5, 0.5], [0.5, -0.5, 0.5], [0.5, 0.5, 0.5], [-0.5, 0.5, 0.5]]),
        ([0.0, 0.0, -1.0], [[-0.5, -0.5, -0.5], [-0.5, 0.5, -0.5], [0.5, 0.5, -0.5], [0.5, -0.5, -0.5]]),
    ];

    let mut verts = Vec::with_capacity(6 * 4 * 6);
    let mut indices = Vec::with_capacity(6 * 6);
    for (normal, corners) in faces {
        let base = (verts.len() / 6) as u32;
        for corner in corners {
            verts.extend_from_slice(&corner);
            verts.extend_from_slice(&normal);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (verts, indices)
}

/// Reinterprets a `T` slice as bytes for `gl.buffer_data_*`.
fn as_bytes<T: Copy>(v: &[T]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), v.len() * std::mem::size_of::<T>())
    }
}
