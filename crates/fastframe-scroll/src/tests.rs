use egui::{Context, Event, Modifiers, MouseWheelUnit, RawInput, TouchPhase, Vec2, vec2};

use super::*;

/// One wheel event.
fn wheel(unit: MouseWheelUnit, delta: Vec2, phase: TouchPhase, modifiers: Modifiers) -> Event {
    Event::MouseWheel {
        unit,
        delta,
        phase,
        modifiers,
    }
}

/// A touchpad movement straight down the pad.
fn touch(phase: TouchPhase, y: f32) -> Event {
    wheel(MouseWheelUnit::Point, vec2(0.0, y), phase, Modifiers::NONE)
}

/// Runs one frame at `time` and returns its smooth scroll delta after
/// `apply`.
fn frame(ctx: &Context, scrolling: &mut Scrolling, time: f64, events: Vec<Event>) -> Vec2 {
    let mut delta = Vec2::ZERO;
    let input = RawInput {
        time: Some(time),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        scrolling.apply(ui.ctx());
        delta = ui.input(|input| input.smooth_scroll_delta);
    });
    output.textures_delta.clear();
    delta
}

/// A fast swipe down the pad over six frames, the first in `phase`.
fn swipe(ctx: &Context, scrolling: &mut Scrolling, phase: TouchPhase) {
    frame(ctx, scrolling, 0.0, vec![touch(phase, 20.0)]);
    for at in 1..6 {
        frame(
            ctx,
            scrolling,
            f64::from(at) * 0.016,
            vec![touch(TouchPhase::Move, 20.0)],
        );
    }
}

fn linux() -> Scrolling {
    Scrolling::with_touchpad_help(true)
}

#[test]
fn a_wheel_notch_scrolls_the_wheel_step_unless_the_app_chose_one() {
    let ctx = Context::default();
    frame(&ctx, &mut Scrolling::default(), 0.0, Vec::new());
    assert_eq!(
        ctx.options(|o| o.input_options.line_scroll_speed),
        WHEEL_STEP
    );

    let ctx = Context::default();
    ctx.options_mut(|o| o.input_options.line_scroll_speed = 60.0);
    frame(&ctx, &mut Scrolling::default(), 0.0, Vec::new());
    assert_eq!(ctx.options(|o| o.input_options.line_scroll_speed), 60.0);
}

#[test]
fn wheel_notches_can_change_direction_without_waiting_for_a_gesture_gap() {
    let ctx = Context::default();
    let mut scrolling = linux();
    let notches = [
        (Modifiers::NONE, vec2(0.0, -3.0), false),
        (Modifiers::SHIFT, vec2(0.0, -3.0), true),
        (Modifiers::NONE, vec2(0.0, -3.0), false),
        (Modifiers::NONE, vec2(-3.0, 0.0), true),
    ];
    for (at, (modifiers, delta, sideways)) in notches.into_iter().enumerate() {
        let event = wheel(MouseWheelUnit::Line, delta, TouchPhase::Move, modifiers);
        let scrolled = frame(&ctx, &mut scrolling, at as f64 / 60.0, vec![event]);
        if sideways {
            assert!(scrolled.x < 0.0, "notch {at} goes sideways: {scrolled:?}");
            assert_eq!(scrolled.y, 0.0, "notch {at}");
        } else {
            assert!(scrolled.y < 0.0, "notch {at} goes down: {scrolled:?}");
            assert_eq!(scrolled.x, 0.0, "notch {at}");
        }
    }
    assert!(!scrolling.from_trackpad());
}

#[test]
fn a_linux_touchpad_scrolls_further_than_its_deltas() {
    // The second frame: egui scrolls nothing on the event that starts a
    // gesture.
    let second_frame = |mut scrolling: Scrolling| {
        let ctx = Context::default();
        frame(
            &ctx,
            &mut scrolling,
            0.0,
            vec![touch(TouchPhase::Start, 10.0)],
        );
        let scrolled = frame(
            &ctx,
            &mut scrolling,
            0.016,
            vec![touch(TouchPhase::Move, 10.0)],
        );
        assert!(scrolling.from_trackpad());
        scrolled.y
    };
    assert_eq!(second_frame(Scrolling::with_touchpad_help(false)), 10.0);
    assert_eq!(second_frame(linux()), 10.0 * TOUCHPAD_SCALE);
}

#[test]
fn a_touchpad_flick_glides_after_the_lift_and_slows_to_a_stop() {
    let ctx = Context::default();
    let mut scrolling = linux();
    swipe(&ctx, &mut scrolling, TouchPhase::Start);
    frame(&ctx, &mut scrolling, 0.1, vec![touch(TouchPhase::End, 0.0)]);
    assert!(scrolling.gliding());

    let mut steps = Vec::new();
    let mut at = 0.1;
    while scrolling.gliding() && steps.len() < 1000 {
        at += 1.0 / 60.0;
        steps.push(frame(&ctx, &mut scrolling, at, Vec::new()).y);
    }
    assert!(!scrolling.gliding(), "the glide stops");
    assert!(steps[0] > 0.0, "it carries on the way the fingers went");
    assert!(
        steps.windows(2).all(|pair| pair[1] <= pair[0]),
        "it slows down"
    );
}

#[test]
fn resting_fingers_do_not_glide_where_the_lift_is_announced() {
    // Wayland says when fingers touch and lift (#503 in Spotifast).
    let ctx = Context::default();
    let mut scrolling = linux();
    swipe(&ctx, &mut scrolling, TouchPhase::Start);
    frame(&ctx, &mut scrolling, 0.3, Vec::new());
    assert!(!scrolling.gliding(), "resting is not a lift");
    frame(&ctx, &mut scrolling, 1.0, vec![touch(TouchPhase::End, 0.0)]);
    assert!(!scrolling.gliding(), "a lift after resting has no speed");
}

#[test]
fn a_quiet_gap_glides_where_the_lift_is_never_announced() {
    // X11 sends every touchpad delta as a plain movement.
    let ctx = Context::default();
    let mut scrolling = linux();
    swipe(&ctx, &mut scrolling, TouchPhase::Move);
    frame(&ctx, &mut scrolling, 0.3, Vec::new());
    assert!(scrolling.gliding());
}

#[test]
fn a_slow_frame_is_no_pause_in_the_gesture() {
    // Pauses are measured on the frames' input clock, not the wall clock: a
    // frame that took long to draw, 16 ms of input after the last, changes
    // nothing about the gesture (ZapFast 99adc7e).
    let ctx = Context::default();
    let mut scrolling = linux();
    swipe(&ctx, &mut scrolling, TouchPhase::Move);
    frame(&ctx, &mut scrolling, 6.0 * 0.016, Vec::new());
    assert!(!scrolling.gliding(), "a slow frame lets the gesture glide");
    // Sideways input just after keeps the vertical axis.
    let sideways = wheel(
        MouseWheelUnit::Point,
        vec2(30.0, 0.0),
        TouchPhase::Move,
        Modifiers::NONE,
    );
    let scrolled = frame(&ctx, &mut scrolling, 7.0 * 0.016, vec![sideways]);
    assert_eq!(scrolled.x, 0.0, "the gesture holds its axis");
}

#[test]
fn a_redone_pass_neither_ends_the_gesture_nor_glides_again() {
    // egui redoes a discarded pass without the frame's events (ZapFast
    // 11eff4d).
    let ctx = Context::default();
    let mut scrolling = linux();
    swipe(&ctx, &mut scrolling, TouchPhase::Move);
    let mut redone_delta = None;
    let input = RawInput {
        time: Some(0.5),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        let ctx = ui.ctx().clone();
        if ctx.current_pass_index() == 0 {
            ctx.request_discard("measured rows above the view");
        } else {
            scrolling.apply(&ctx);
            redone_delta = Some(ctx.input(|input| input.smooth_scroll_delta));
        }
    });
    output.textures_delta.clear();
    assert_eq!(redone_delta, Some(Vec2::ZERO), "the redone pass scrolls");
    assert!(!scrolling.gliding(), "the redone pass ended the gesture");
}

#[test]
fn a_sideways_modifier_turns_the_gesture_and_its_glide() {
    let ctx = Context::default();
    let mut scrolling = linux();
    let shifted = |phase| {
        wheel(
            MouseWheelUnit::Point,
            vec2(0.0, 20.0),
            phase,
            Modifiers::SHIFT,
        )
    };
    frame(&ctx, &mut scrolling, 0.0, vec![shifted(TouchPhase::Start)]);
    for at in 1..6 {
        let scrolled = frame(
            &ctx,
            &mut scrolling,
            f64::from(at) * 0.016,
            vec![shifted(TouchPhase::Move)],
        );
        assert_eq!(scrolled.y, 0.0, "frame {at} stays sideways");
    }
    frame(&ctx, &mut scrolling, 0.1, vec![touch(TouchPhase::End, 0.0)]);
    let glide = frame(&ctx, &mut scrolling, 0.1 + 1.0 / 60.0, Vec::new());
    assert!(
        glide.x > 0.0 && glide.y == 0.0,
        "the glide goes sideways: {glide:?}"
    );
}

#[test]
fn a_press_stops_a_glide() {
    let ctx = Context::default();
    let mut scrolling = linux();
    swipe(&ctx, &mut scrolling, TouchPhase::Start);
    frame(&ctx, &mut scrolling, 0.1, vec![touch(TouchPhase::End, 0.0)]);
    assert!(scrolling.gliding());
    let press = Event::PointerButton {
        pos: egui::pos2(10.0, 10.0),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    };
    frame(
        &ctx,
        &mut scrolling,
        0.12,
        vec![Event::PointerMoved(egui::pos2(10.0, 10.0)), press],
    );
    assert!(!scrolling.gliding());
}

#[test]
fn elsewhere_the_system_scales_and_glides_touchpads_itself() {
    // macOS adds its own acceleration and momentum before egui sees a delta.
    let ctx = Context::default();
    let mut scrolling = Scrolling::with_touchpad_help(false);
    swipe(&ctx, &mut scrolling, TouchPhase::Start);
    frame(&ctx, &mut scrolling, 0.1, vec![touch(TouchPhase::End, 0.0)]);
    assert!(scrolling.from_trackpad());
    assert!(!scrolling.gliding());
}

#[test]
fn stop_ends_a_glide_and_frees_the_axis() {
    let ctx = Context::default();
    let mut scrolling = linux();
    swipe(&ctx, &mut scrolling, TouchPhase::Start);
    frame(&ctx, &mut scrolling, 0.1, vec![touch(TouchPhase::End, 0.0)]);
    assert!(scrolling.gliding());
    scrolling.stop();
    assert!(!scrolling.gliding());
    assert_eq!(frame(&ctx, &mut scrolling, 0.12, Vec::new()), Vec2::ZERO);
    // A sideways gesture right after is not held to the old axis.
    let sideways = wheel(
        MouseWheelUnit::Point,
        vec2(30.0, 0.0),
        TouchPhase::Start,
        Modifiers::NONE,
    );
    frame(&ctx, &mut scrolling, 0.13, vec![sideways]);
    let sideways = wheel(
        MouseWheelUnit::Point,
        vec2(30.0, 0.0),
        TouchPhase::Move,
        Modifiers::NONE,
    );
    let scrolled = frame(&ctx, &mut scrolling, 0.146, vec![sideways]);
    assert!(scrolled.x > 0.0, "{scrolled:?}");
}

#[test]
fn a_window_made_again_gets_the_wheel_step_too() {
    let mut scrolling = Scrolling::default();
    frame(&Context::default(), &mut scrolling, 0.0, Vec::new());
    let again = Context::default();
    frame(&again, &mut scrolling, 0.016, Vec::new());
    assert_eq!(
        again.options(|o| o.input_options.line_scroll_speed),
        WHEEL_STEP
    );
}
