//! Opens an OpenGL window and renders a loaded `.rbxlx` place with a
//! physics-driven avatar you can walk around in.
//!
//! Usage: `cargo run --example render_place -- [path] [--frames N]`
//!
//! Controls:
//! - WASD: walk, Space: jump
//! - F: disintegrate (the classic parts-scatter death; respawns in ~2.5s)
//! - Left-drag: orbit the camera, Right-drag: pan, Scroll: zoom
//! - R: reset the camera angle, Escape: exit
//!
//! `--frames N` renders N frames and exits; `--die` disintegrates the avatar
//! once after 10 frames, useful for smoke-testing the scatter + respawn path.

use std::num::NonZeroU32;
use std::path::Path;

use glow::HasContext;
use glutin::config::ConfigTemplateBuilder;
use glutin::display::GetGlDisplay;
use glutin::context::{ContextApi, ContextAttributesBuilder, GlProfile, Version};
use glutin::prelude::*;
use glutin::surface::{SurfaceAttributesBuilder, WindowSurface};
use glutin_winit::DisplayBuilder;
use raw_window_handle::HasRawWindowHandle;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Event, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowBuilder;

use bloxvm::physics::Player;
use bloxvm::render::{build_scene, GLRenderer, OrbitCamera};
use bloxvm::rbxlx::DataModel;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut path = "tests/fixtures/minimal.rbxlx".to_string();
    let mut frames_limit: Option<u32> = None;
    let mut die_once = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => frames_limit = args.next().and_then(|s| s.parse().ok()),
            "--die" => die_once = true,
            _ => path = arg,
        }
    }

    let dm = DataModel::parse_rbxlx_path(Path::new(&path))?;
    let scene = build_scene(&dm);
    println!("loaded {} parts", scene.parts.len());
    if let Some((min, max)) = scene.bounds {
        println!("bounds: min = {min:?}, max = {max:?}");
    } else {
        println!("(scene is empty)");
    }

    let mut player = Player::new(&scene, Player::spawn_point(&scene));
    let (spawn, _) = player.character_transform();
    println!("avatar spawned at {spawn:?}");

    let mut camera = OrbitCamera::new();
    camera.center = spawn;

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let window_builder = WindowBuilder::new()
        .with_title("BloxVM")
        .with_inner_size(LogicalSize::new(1280.0, 800.0));

    let display_builder = DisplayBuilder::new().with_window_builder(Some(window_builder));

    let (window, gl_config) = display_builder.build(
        &event_loop,
        ConfigTemplateBuilder::new().with_depth_size(24),
        |mut configs| configs.next().unwrap(),
    )?;
    let window = window.ok_or("glutin created no window (headless platform?)")?;

    let raw_window_handle = window.raw_window_handle();
    let context_attributes = ContextAttributesBuilder::new()
        .with_profile(GlProfile::Core)
        .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
        .build(Some(raw_window_handle));
    // SAFETY: The GL context is created from the picked config; the display and
    // window are valid for the lifetime of the context.
    let not_current = unsafe { gl_config.display().create_context(&gl_config, &context_attributes) }?;

    let inner = window.inner_size();
    let surface_attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(
        raw_window_handle,
        NonZeroU32::new(inner.width.max(1)).unwrap(),
        NonZeroU32::new(inner.height.max(1)).unwrap(),
    );
    // SAFETY: The window surface is created on the same display as the config.
    let surface = unsafe { gl_config.display().create_window_surface(&gl_config, &surface_attrs) }?;
    let gl_context = not_current.make_current(&surface)?;

    // SAFETY: glutin provides GL function pointers via the display's proc
    // address loader; these are valid while the display lives.
    let gl = unsafe { glow::Context::from_loader_function_cstr(|s| gl_config.display().get_proc_address(s)) };
    let renderer = GLRenderer::new(&gl)?;

    println!("OpenGL: {}", unsafe { gl.get_parameter_string(glow::VERSION) });
    println!("rendering {} parts (WASD to walk, Space to jump, F to disintegrate, Escape to quit)", scene.parts.len());

    let mut dragging = false;
    let mut panning = false;
    let mut last_cursor: Option<(f64, f64)> = None;
    let mut keys = (false, false, false, false); // W, A, S, D
    let mut jump_queued = false;
    let mut died_smoke = false;
    let mut rendered = 0u32;
    let mut size = (inner.width, inner.height);

    event_loop.run(move |event, event_loop| {
        match event {
            Event::WindowEvent { event: WindowEvent::Resized(new_size), .. } => {
                let (w, h) = (new_size.width.max(1), new_size.height.max(1));
                surface.resize(&gl_context, NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap());
                size = (w, h);
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => event_loop.exit(),
            Event::WindowEvent {
                event: WindowEvent::KeyboardInput {
                    event: KeyEvent { state, logical_key: key, .. },
                    ..
                },
                ..
            } => match key {
                Key::Named(NamedKey::Escape) => event_loop.exit(),
                Key::Named(NamedKey::Space) if state == ElementState::Pressed => jump_queued = true,
                Key::Character(c) => {
                    let down = state == ElementState::Pressed;
                    match c.as_str() {
                        "w" | "W" => keys.0 = down,
                        "a" | "A" => keys.1 = down,
                        "s" | "S" => keys.2 = down,
                        "d" | "D" => keys.3 = down,
                        "r" | "R" if down => {
                            camera.yaw = -0.7;
                            camera.pitch = -0.45;
                            camera.distance = 14.0;
                        }
                        "f" | "F" if down => player.die(),
                        _ => {}
                    }
                }
                _ => {}
            },
            Event::WindowEvent { event: WindowEvent::MouseInput { state, button, .. }, .. } => {
                match button {
                    MouseButton::Left => {
                        dragging = state == ElementState::Pressed;
                        if dragging {
                            last_cursor = None;
                        }
                    }
                    MouseButton::Right => {
                        panning = state == ElementState::Pressed;
                        if panning {
                            last_cursor = None;
                        }
                    }
                    _ => {}
                }
            }
            Event::WindowEvent { event: WindowEvent::CursorMoved { position, .. }, .. } => {
                if dragging || panning {
                    if let Some((lx, ly)) = last_cursor {
                        let (dx, dy) = ((position.x - lx) as f32, (position.y - ly) as f32);
                        if dragging {
                            camera.drag(dx, dy);
                        } else {
                            camera.pan(dx, -dy, 0.0);
                        }
                    }
                }
                last_cursor = Some((position.x, position.y));
            }
            Event::WindowEvent { event: WindowEvent::MouseWheel { delta, .. }, .. } => {
                let y = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32,
                };
                camera.zoom(y);
            }
            Event::AboutToWait => {
                window.request_redraw();
            }
            Event::WindowEvent { event: WindowEvent::RedrawRequested, .. } => {
                let (w, a, s, d) = keys;
                let right = (d as i8 - a as i8) as f32;
                let forward = (w as i8 - s as i8) as f32;
                player.set_move_input(right, forward, camera.yaw);
                if jump_queued {
                    player.try_jump();
                    jump_queued = false;
                }
                player.step();
                let (pos, _) = player.character_transform();
                camera.center = pos;
                if die_once && !died_smoke && rendered >= 10 {
                    player.die();
                    died_smoke = true;
                    println!("disintegrated at frame {rendered}");
                }

                let mut parts = scene.parts.clone();
                parts.extend(player.avatar_parts());
                renderer.render(&gl, &camera, size.0, size.1, &parts);
                let _ = surface.swap_buffers(&gl_context);
                rendered += 1;
                if let Some(limit) = frames_limit {
                    if rendered >= limit {
                        println!("rendered {rendered} frames; exiting");
                        event_loop.exit();
                    }
                }
            }
            _ => {}
        }
    })?;

    Ok(())
}
