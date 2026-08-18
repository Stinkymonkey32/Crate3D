//! OpenGL renderer (via `glow`).
//!
//! Builds a render scene from a loaded [`DataModel`], frames an orbit camera
//! around it, and draws every `BasePart` as a flat-shaded colored box. The
//! window/input handling lives in the examples; this module only touches GL.

use std::collections::BTreeMap;

use glam::{Mat3, Mat4, Vec3};
use glow::HasContext;

use crate::instance::{DataModel, Instance};
use crate::texture::{self, TextureManager};
use crate::value::Value;

/// Where a part's texture comes from.
#[derive(Debug, Clone)]
pub enum TextureSource {
    /// Embedded in the place file as a SharedString, keyed by MD5.
    SharedString([u8; 16]),
    /// Referenced by a Content URI, to be downloaded.
    Content(String),
}

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
    /// Texture source: embedded SharedString or external Content URI.
    /// `None` = use flat color only.
    pub texture_source: Option<TextureSource>,
    /// How many studs per texture tile (U, V).
    /// Inherited from `Texture.StudsPerTileU/V` or `Decal` defaults.
    pub studs_per_tile: [f32; 2],
    /// UV offset from `Texture.UVOffset` / `Decal` properties.
    pub uv_offset: [f32; 2],
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

        let surface = find_surface_info(dm, inst, &size);
        let texture_source = surface.as_ref().map(|s| s.texture_source.clone());
        let studs_per_tile = surface.as_ref().map(|s| s.studs_per_tile).unwrap_or([1.0, 1.0]);
        let uv_offset = surface.as_ref().map(|s| s.uv_offset).unwrap_or([0.0, 0.0]);

        parts.push(RenderPart {
            name: inst.name.clone(),
            position,
            rotation,
            size,
            color,
            texture_source,
            studs_per_tile,
            uv_offset,
        });
    }

    let bounds = compute_bounds(&parts);
    Scene { parts, bounds }
}

/// Result of scanning a part's children for texture info.
struct SurfaceInfo {
    texture_source: TextureSource,
    studs_per_tile: [f32; 2],
    uv_offset: [f32; 2],
}

/// Looks up the texture source and UV tiling for a `BasePart` instance by
/// scanning its children (`SurfaceAppearance`, `Texture`, `Decal`) and its
/// own `Texture` property. `part_size` is the part's Size in studs, used to
/// default studs_per_tile for Decals/SurfaceAppearance so the texture fills
/// the face exactly once.
fn find_surface_info(dm: &DataModel, inst: &Instance, part_size: &[f32; 3]) -> Option<SurfaceInfo> {
    for &child_id in &inst.children {
        let child = &dm.instances[child_id];
        match child.class.as_str() {
            "SurfaceAppearance" => {
                if let Some(Value::SharedString { key, .. }) = child.get_property("ColorMap") {
                    if let Some(md5) = texture::parse_md5_hex(&key) {
                        return Some(SurfaceInfo {
                            texture_source: TextureSource::SharedString(md5),
                            studs_per_tile: [part_size[0], part_size[2]],
                            uv_offset: [0.0, 0.0],
                        });
                    }
                }
                if let Some(Value::Content(uri)) = child.get_property("ColorMap") {
                    if !uri.is_empty() {
                        return Some(SurfaceInfo {
                            texture_source: TextureSource::Content(uri.clone()),
                            studs_per_tile: [part_size[0], part_size[2]],
                            uv_offset: [0.0, 0.0],
                        });
                    }
                }
            }
            "Texture" => {
                if let Some(Value::Content(uri)) = child.get_property("Texture") {
                    if !uri.is_empty() {
                        let spt_u = child.get_property("StudsPerTileU")
                            .and_then(|v| match v { Value::Float(f) => Some(*f), Value::Double(f) => Some(*f as f32), _ => None })
                            .unwrap_or(1.0).max(0.01);
                        let spt_v = child.get_property("StudsPerTileV")
                            .and_then(|v| match v { Value::Float(f) => Some(*f), Value::Double(f) => Some(*f as f32), _ => None })
                            .unwrap_or(1.0).max(0.01);
                        let uv_off = read_vec2(child, "UVOffset").unwrap_or([0.0, 0.0]);
                        return Some(SurfaceInfo {
                            texture_source: TextureSource::Content(uri.clone()),
                            studs_per_tile: [spt_u, spt_v],
                            uv_offset: uv_off,
                        });
                    }
                }
            }
            "Decal" => {
                if let Some(Value::Content(uri)) = child.get_property("Texture") {
                    if !uri.is_empty() {
                        let uv_off = read_vec2(child, "UVOffset").unwrap_or([0.0, 0.0]);
                        return Some(SurfaceInfo {
                            texture_source: TextureSource::Content(uri.clone()),
                            studs_per_tile: [part_size[0], part_size[2]],
                            uv_offset: uv_off,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    // Fall back to the part's own Texture property (legacy Content URI).
    if let Some(Value::Content(uri)) = inst.get_property("Texture") {
        if !uri.is_empty() {
            return Some(SurfaceInfo {
                texture_source: TextureSource::Content(uri.clone()),
                studs_per_tile: [part_size[0], part_size[2]],
                uv_offset: [0.0, 0.0],
            });
        }
    }

    None
}

/// Reads a `Vector2` property from an instance, returning `[x, y]`.
fn read_vec2(inst: &Instance, name: &str) -> Option<[f32; 2]> {
    match inst.get_property(name)? {
        Value::Vector2 { x, y } => Some([*x, *y]),
        _ => None,
    }
}

/// Pre-uploads every DDS blob found in the place file's `<SharedStrings>`
/// block to the GPU. Non-DDS blobs are silently skipped.
pub fn prepare_textures(gl: &glow::Context, dm: &DataModel, tex_manager: &mut TextureManager) {
    for (key_str, bytes) in &dm.shared_strings {
        if let Some(key) = texture::parse_md5_hex(key_str) {
            if let Some(tex) = texture::parse_dds(bytes) {
                tex_manager.upload(gl, &key, &tex);
            }
        }
    }
}

/// Uploads downloaded Content texture bytes to the GPU. The `content_cache`
/// maps Content URI strings to their downloaded bytes. DDS blobs are parsed
/// and uploaded to the texture manager keyed by the URI (so parts referencing
/// the same URI share one GPU texture).
pub fn prepare_content_textures(
    gl: &glow::Context,
    content_cache: &BTreeMap<String, Vec<u8>>,
    tex_manager: &mut TextureManager,
) {
    for (uri, bytes) in content_cache {
        let magic = if bytes.len() >= 4 {
            format!("{:02x} {:02x} {:02x} {:02x}", bytes[0], bytes[1], bytes[2], bytes[3])
        } else {
            format!("{} bytes", bytes.len())
        };
        let tex = texture::parse_dds(bytes)
            .or_else(|| texture::parse_png(bytes))
            .or_else(|| {
                image::load_from_memory(bytes).ok().map(|img| {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    texture::Texture {
                        width: w,
                        height: h,
                        depth: 1,
                        mip_levels: 1,
                        format: texture::TextureFormat::Rgba8,
                        data: rgba.into_raw(),
                    }
                })
            });
        match tex {
            Some(tex) => {
                println!("  [content] {uri}: {}x{} {:?} (magic: {})", tex.width, tex.height, tex.format, magic);
                let key = md5_string_key(uri);
                tex_manager.upload(gl, &key, &tex);
            }
            None => {
                println!("  [content] {uri}: FAILED to parse (magic: {}, {} bytes)", magic, bytes.len());
            }
        }
    }
}

/// Maps a string key (Content URI or any identifier) to a `[u8; 16]` by
/// hashing the bytes with a simple FNV-1a. This is NOT cryptographically
/// secure — it's just for deduplication in the texture cache.
fn md5_string_key(s: &str) -> [u8; 16] {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    let h = hasher.finish();
    // Fill the 16-byte key with the 8-byte hash (padded with zeros).
    let mut key = [0u8; 16];
    key[..8].copy_from_slice(&h.to_le_bytes());
    // Mix the upper bytes to reduce collision risk.
    key[8..16].copy_from_slice(&(h.reverse_bits()).to_le_bytes());
    key
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
    u_has_texture: Option<glow::UniformLocation>,
    u_size: Option<glow::UniformLocation>,
    u_studs_per_tile: Option<glow::UniformLocation>,
    u_uv_offset: Option<glow::UniformLocation>,
}

const VERTEX_SRC: &str = r#"#version 330 core
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec3 a_normal;
layout(location = 2) in vec2 a_uv;
uniform mat4 u_mvp;
uniform mat3 u_normal_mat;
out vec3 v_normal;
out vec3 v_obj_pos;
out vec2 v_uv;
void main() {
    v_normal = u_normal_mat * a_normal;
    v_obj_pos = a_pos;
    v_uv = a_uv;
    gl_Position = u_mvp * vec4(a_pos, 1.0);
}
"#;

const FRAGMENT_SRC: &str = r#"#version 330 core
in vec3 v_normal;
in vec3 v_obj_pos;
in vec2 v_uv;
uniform vec4 u_color;
uniform vec3 u_light_dir;
uniform vec3 u_size;
uniform vec2 u_studs_per_tile;
uniform vec2 u_uv_offset;
uniform sampler2D u_texture;
uniform bool u_has_texture;
out vec4 frag_color;
void main() {
    vec3 n = normalize(v_normal);
    float diff = max(dot(n, normalize(u_light_dir)), 0.0);
    float intensity = 0.15 + diff * 0.85;

    vec2 uv;
    if (u_has_texture) {
        // Box-projected UVs: pick face axes from the dominant normal component.
        vec3 p = v_obj_pos + 0.5;
        vec3 abs_n = abs(n);
        if (abs_n.y >= abs_n.x && abs_n.y >= abs_n.z) {
            uv = p.xz * u_size.xz / u_studs_per_tile;
        } else if (abs_n.x >= abs_n.z) {
            uv = p.zy * u_size.zy / u_studs_per_tile;
        } else {
            uv = p.xy * u_size.xy / u_studs_per_tile;
        }
        uv += u_uv_offset;
    } else {
        uv = v_uv;
    }

    vec3 base_color = u_has_texture
        ? texture(u_texture, uv).rgb
        : u_color.rgb;
    frag_color = vec4(base_color * intensity, u_color.a);
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

            // a_pos: 3 floats at offset 0
            // a_normal: 3 floats at offset 12
            // a_uv: 2 floats at offset 24
            let stride = 32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 12);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 2, glow::FLOAT, false, stride, 24);
        }

        unsafe {
            gl.use_program(Some(program));
            let light = Vec3::new(0.4, 0.8, 0.6).normalize();
            gl.uniform_3_f32(gl.get_uniform_location(program, "u_light_dir").as_ref(), light.x, light.y, light.z);
            // Set the texture sampler to read from texture unit 0.
            gl.uniform_1_i32(
                gl.get_uniform_location(program, "u_texture").as_ref(),
                0,
            );
            gl.enable(glow::DEPTH_TEST);
        }

        Ok(GLRenderer {
            program,
            vao,
            index_count: indices.len() as i32,
            u_mvp: unsafe { gl.get_uniform_location(program, "u_mvp") },
            u_normal_mat: unsafe { gl.get_uniform_location(program, "u_normal_mat") },
            u_color: unsafe { gl.get_uniform_location(program, "u_color") },
            u_has_texture: unsafe { gl.get_uniform_location(program, "u_has_texture") },
            u_size: unsafe { gl.get_uniform_location(program, "u_size") },
            u_studs_per_tile: unsafe { gl.get_uniform_location(program, "u_studs_per_tile") },
            u_uv_offset: unsafe { gl.get_uniform_location(program, "u_uv_offset") },
        })
    }

    /// Clears to the sky color and draws every part.
    pub fn render(&self, gl: &glow::Context, camera: &OrbitCamera, width: u32, height: u32, parts: &[RenderPart], tex_manager: &TextureManager) {
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
            gl.active_texture(glow::TEXTURE0);
        }

        for part in parts {
            self.draw_part(gl, &view, &proj, part, tex_manager);
        }
    }

    fn draw_part(&self, gl: &glow::Context, view: &Mat4, proj: &Mat4, part: &RenderPart, tex_manager: &TextureManager) {
        let rot = Mat3::from_cols_array_2d(&[
            [part.rotation[0], part.rotation[1], part.rotation[2]],
            [part.rotation[3], part.rotation[4], part.rotation[5]],
            [part.rotation[6], part.rotation[7], part.rotation[8]],
        ]);
        let model = Mat4::from_scale_rotation_translation(Vec3::from(part.size), glam::Quat::from_mat3(&rot), Vec3::from(part.position));
        let mvp = *proj * *view * model;
        let normal_mat = Mat3::from_mat4(model.inverse().transpose());

        let (has_tex, tex_handle) = match &part.texture_source {
            Some(TextureSource::SharedString(md5)) => (true, tex_manager.handle(md5)),
            Some(TextureSource::Content(uri)) => {
                let key = md5_string_key(uri);
                (true, tex_manager.handle(&key))
            }
            None => (false, tex_manager.fallback()),
        };

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
            gl.uniform_3_f32(self.u_size.as_ref(), part.size[0], part.size[1], part.size[2]);
            gl.uniform_2_f32(self.u_studs_per_tile.as_ref(), part.studs_per_tile[0], part.studs_per_tile[1]);
            gl.uniform_2_f32(self.u_uv_offset.as_ref(), part.uv_offset[0], part.uv_offset[1]);
            gl.bind_texture(glow::TEXTURE_2D, Some(tex_handle));
            gl.uniform_1_i32(self.u_has_texture.as_ref(), has_tex as i32);
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

/// Builds a unit cube ([-0.5, 0.5]^3) as interleaved pos/normal/uv vertices.
fn unit_cube() -> (Vec<f32>, Vec<u32>) {
    // Each face: normal, 4 corners (pos), and their UV coordinates.
    let faces: [([f32; 3], [[f32; 3]; 4], [[f32; 2]; 4]); 6] = [
        // +X
        ([1.0, 0.0, 0.0],
         [[0.5, -0.5, -0.5], [0.5, 0.5, -0.5], [0.5, 0.5, 0.5], [0.5, -0.5, 0.5]],
         [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]),
        // -X
        ([-1.0, 0.0, 0.0],
         [[-0.5, -0.5, 0.5], [-0.5, 0.5, 0.5], [-0.5, 0.5, -0.5], [-0.5, -0.5, -0.5]],
         [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]),
        // +Y
        ([0.0, 1.0, 0.0],
         [[-0.5, 0.5, -0.5], [-0.5, 0.5, 0.5], [0.5, 0.5, 0.5], [0.5, 0.5, -0.5]],
         [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]]),
        // -Y
        ([0.0, -1.0, 0.0],
         [[-0.5, -0.5, 0.5], [-0.5, -0.5, -0.5], [0.5, -0.5, -0.5], [0.5, -0.5, 0.5]],
         [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]]),
        // +Z
        ([0.0, 0.0, 1.0],
         [[-0.5, -0.5, 0.5], [0.5, -0.5, 0.5], [0.5, 0.5, 0.5], [-0.5, 0.5, 0.5]],
         [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]),
        // -Z
        ([0.0, 0.0, -1.0],
         [[-0.5, -0.5, -0.5], [0.5, -0.5, -0.5], [0.5, 0.5, -0.5], [-0.5, 0.5, -0.5]],
         [[1.0, 0.0], [0.0, 0.0], [0.0, 1.0], [1.0, 1.0]]),
    ];

    let mut verts = Vec::with_capacity(6 * 4 * 8);
    let mut indices = Vec::with_capacity(6 * 6);
    for (normal, corners, uvs) in faces {
        let base = (verts.len() / 8) as u32;
        for (corner, uv) in corners.iter().zip(uvs.iter()) {
            verts.extend_from_slice(corner);
            verts.extend_from_slice(&normal);
            verts.extend_from_slice(uv);
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
