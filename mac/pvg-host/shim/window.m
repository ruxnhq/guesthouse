// SPDX-License-Identifier: GPL-2.0-or-later
//
// A window on the Mac that shows a guest's picture, as plain C functions for src/window.rs.
//
// The window holds a Metal layer, which guesthouse-pvg-sys has the guest's display draw its
// frames into, and over it a small layer for the guest's cursor. The layer keeps the picture's
// shape: a picture of another size than the window is scaled to fit, with bars beside it.
//
// With input, the window also takes the keyboard and the pointer for the guest. Keys, modifiers,
// pointer moves, buttons and the wheel go to src/window.rs as they come, from an event monitor of
// this process's own: it sees only what AppKit hands this process, before the window does, and
// so also the keys that AppKit keeps from a window while Command is held. When the window stops
// being the key window, every key and button is let go. While the pointer is over the picture,
// the Mac's pointer takes the guest's cursor's picture, and the cursor's layer is hidden.
//
// AppKit belongs to the process's first thread, so pvg_window_open and pvg_window_run are called
// there, and every other function hands its work to that thread.

#import <AppKit/AppKit.h>
#import <Carbon/Carbon.h>
#import <QuartzCore/QuartzCore.h>
#include <stdatomic.h>
#include <time.h>

typedef struct {
   // The window's content has a new size, in pixels of the screen.
   void (*resized)(uint32_t width, uint32_t height);
   // A key went down or up, by the Mac's virtual key code.
   void (*key)(uint16_t code, bool down);
   // The pointer is at a point of the window's content, in points from its top left, of a content
   // of this size, over a picture of this many pixels.
   void (*pointer)(double x, double y, double width, double height, uint32_t picture_width,
                   uint32_t picture_height);
   // A button went down or up: 0 left, 1 right, 2 middle, 3 and 4 the side buttons.
   void (*button)(uint32_t button, bool down);
   // The wheel turned by steps, to the right and up.
   void (*wheel)(int32_t right, int32_t up);
   // Every key and button is to be let go.
   void (*released)(void);
   // The window's close button was pressed. Returns whether the guest was asked to power down.
   bool (*closing)(void);
} pvg_window_callbacks;

// How the window takes input.
enum {
   PVG_INPUT_NONE = 0,
   // Keys and pointer, with the window in front and the keyboard's.
   PVG_INPUT_FOCUS = 1,
   // The same, from pvg_window_test_event only: the window stays where it was opened and never
   // takes the keyboard, for runs with no person.
   PVG_INPUT_TEST = 2,
};

// The test events of pvg_window_test_event.
enum {
   PVG_TEST_KEY = 0,
   PVG_TEST_FLAGS = 1,
   PVG_TEST_MOVE = 2,
   PVG_TEST_BUTTON = 3,
   PVG_TEST_SCROLL = 4,
   PVG_TEST_RESIGN = 5,
   PVG_TEST_CLOSE = 6,
   PVG_TEST_EDGE = 7,
   PVG_TEST_DRAG = 8,
   PVG_TEST_SCREEN = 9,
};

@interface PVGWindowDelegate : NSObject <NSWindowDelegate>
@end

@interface PVGView : NSView
@end

static NSWindow *window;
static PVGView *view;
static CAMetalLayer *picture;
static CALayer *cursor;
static PVGWindowDelegate *delegate;
static pvg_window_callbacks callbacks;
static int input;
static id monitor;
// The guest's picture, and its cursor's place and picture, as last told.
static CGSize picture_size;
static CGPoint cursor_place, cursor_hot;
static CGSize cursor_size;
static bool cursor_shown;
static CGImageRef cursor_image;
// The Mac's pointer while it is over the picture, made from the guest's cursor at the scale it
// was made for, and one that shows nothing for a guest that hides its cursor.
static NSCursor *guest_pointer, *no_pointer;
static CGFloat guest_pointer_scale;
static bool pointer_inside;
// What the wheel turned by that is not a whole step yet.
static double wheel_right, wheel_up;
// The user drags the window's edge or corner: AppKit's loop for it takes the drag's events, the
// button's release among them, and they never reach the monitor.
static bool live_resize;
// An event of pvg_window_test_event is being handed over. With PVG_INPUT_TEST the window takes
// these only, and none of a person's who happens to touch it. A test's drag handed to AppKit
// itself comes back through the monitor later, from the queue, numbered anew (seen): its moves
// and release are taken while it lasts.
static bool made_up;
static bool made_up_drag;
// A left press near the content's left, right or bottom edge, where AppKit may take it for the
// start of a drag of the window's size. AppKit decides once, at the drag's first move that has
// gone 1 point from the press on either axis: that move comes through the monitor, and then the
// live resize starts, or never for this drag (seen with events made up in the process,
// 2026-11-27, macOS 27.2.1). So the guest gets the press only at the next event through the
// monitor after a move that has gone START_DISTANCE, when no live resize has started by then. A
// press let go before that, a click on the edge, never reaches the guest, nor does its release;
// when a live resize starts, the press is dropped.
static bool press_held;
static NSPoint held_at;
static bool held_far;
// Where AppKit starts a drag of the size from a press on the content, in points (seen as above,
// on a window like this one): up to 3 from the left, right and bottom edges, and in a rounded
// square of about 15 at each bottom corner. A press is held back a little beyond that.
static const CGFloat RESIZE_EDGE = 4, RESIZE_CORNER = 16;
// How far a held press's drag goes before the guest may get it, in points on either axis: twice
// AppKit's distance, which was seen on a screen of scale 2 only. A larger one only delays the press.
static const CGFloat START_DISTANCE = 2;
// The smallest content in pixels: src/host.rs offers the guest no smaller mode.
static const CGFloat SMALLEST_PIXELS = 320;
// Points of a trackpad's precise scrolling to a step of the wheel.
static const double POINTS_A_STEP = 16;

// How the first thread is doing, for src/watch.rs: when it last ran the work of pvg_window_ping,
// and which function of this file it is in, and which it last left. A function marks itself with
// MAIN_STEP at its start; leaving it, by any way, gives the step back to the one it was called in.
static _Atomic uint64_t main_answered;
static _Atomic(const char *) main_in, main_left;

typedef struct {
   const char *name, *outer;
} main_step;

static void
main_step_end(main_step *step)
{
   atomic_store(&main_in, step->outer);
   atomic_store(&main_left, step->name);
}

#define MAIN_STEP(name)                                                                            \
   __attribute__((cleanup(main_step_end))) main_step step_ = {name, atomic_exchange(&main_in, name)}

// The window as the first thread last saw it, for pvg_window_state, which waits for nobody: the
// thread that serves the guest asks it, and a guest's register access waits on that thread.
static _Atomic uint32_t kept_width, kept_height;
static _Atomic bool kept_visible, kept_cursor;

static void
keep_state(void)
{
   NSSize pixels = [view convertSizeToBacking:view.bounds.size];
   atomic_store(&kept_width, (uint32_t)pixels.width);
   atomic_store(&kept_height, (uint32_t)pixels.height);
   atomic_store(&kept_visible, window.visible && (window.occlusionState & NSWindowOcclusionStateVisible));
   atomic_store(&kept_cursor, !cursor.hidden && cursor.contents != nil);
}

// The content's pixels the window keeps when it goes to a screen of another scale: its size before
// the move, not what AppKit made of it on the way.
static NSSize pixels_kept;

static void
tell_size(void)
{
   MAIN_STEP("tell_size");
   NSSize pixels = [view convertSizeToBacking:view.bounds.size];
   keep_state();
   // On a screen whose scale the window has not taken yet, a move between screens of two scales
   // is under way: AppKit may first hold the window's points to the new screen, at the old scale,
   // and the change of scale after it gives the size (seen 2026-11-27, macOS 27.2.1, the PD34 at
   // scale 1 and the Mac's own screen at 2). That passing size is not the guest's.
   if (window.screen && window.screen.backingScaleFactor != window.backingScaleFactor)
      return;
   pixels_kept = pixels;
   callbacks.resized((uint32_t)pixels.width, (uint32_t)pixels.height);
}

// How many points of the window a pixel of the picture is, and where the picture's left and
// bottom edges are.
static CGFloat
picture_scale(CGFloat *left, CGFloat *bottom)
{
   CGSize bounds = view.bounds.size;
   CGFloat scale = MIN(bounds.width / picture_size.width, bounds.height / picture_size.height);
   *left = (bounds.width - picture_size.width * scale) / 2;
   *bottom = (bounds.height - picture_size.height * scale) / 2;
   return scale;
}

// Whether the Mac's pointer is to show the guest's cursor now.
static bool
pointer_is_guests(void)
{
   return input != PVG_INPUT_NONE && pointer_inside && cursor_image != NULL;
}

// The Mac's pointer over the picture: the guest's cursor at the picture's scale, or nothing
// while the guest hides it, or the Mac's own arrow while the guest has shown none.
static NSCursor *
pointer_now(void)
{
   MAIN_STEP("pointer_now");
   if (!cursor_image)
      return NSCursor.arrowCursor;
   if (!cursor_shown)
      return no_pointer;
   CGFloat left, bottom;
   CGFloat scale = picture_scale(&left, &bottom);
   if (!guest_pointer || scale != guest_pointer_scale) {
      NSSize points = NSMakeSize(cursor_size.width * scale, cursor_size.height * scale);
      NSImage *image = [[NSImage alloc] initWithCGImage:cursor_image size:points];
      guest_pointer = [[NSCursor alloc] initWithImage:image
                                              hotSpot:NSMakePoint(cursor_hot.x * scale, cursor_hot.y * scale)];
      guest_pointer_scale = scale;
   }
   return guest_pointer;
}

// Puts the cursor's layer where the guest's cursor is, in the scaled picture.
static void
place_cursor(void)
{
   MAIN_STEP("place_cursor");
   if (!picture_size.width || !picture_size.height || !cursor_size.width) {
      cursor.hidden = YES;
      keep_state();
      return;
   }
   CGFloat left, bottom;
   CGFloat scale = picture_scale(&left, &bottom);
   [CATransaction begin];
   [CATransaction setDisableActions:YES];
   cursor.bounds = CGRectMake(0, 0, cursor_size.width * scale, cursor_size.height * scale);
   // The guest counts rows from the top, a layer from the bottom.
   cursor.anchorPoint = CGPointMake(cursor_hot.x / cursor_size.width, 1 - cursor_hot.y / cursor_size.height);
   cursor.position = CGPointMake(left + cursor_place.x * scale,
                                 bottom + (picture_size.height - cursor_place.y) * scale);
   cursor.hidden = !cursor_shown || pointer_is_guests();
   [CATransaction commit];
   keep_state();
   if (pointer_inside && input != PVG_INPUT_NONE)
      [pointer_now() set];
}

// The pointer is at this point of the window, in its own coordinates, from the bottom left.
static void
point_at(NSPoint where)
{
   MAIN_STEP("point_at");
   NSPoint local = [view convertPoint:where fromView:nil];
   NSSize size = view.bounds.size;
   if (!picture_size.width || !picture_size.height)
      return;
   callbacks.pointer(local.x, size.height - local.y, size.width, size.height, (uint32_t)picture_size.width,
                     (uint32_t)picture_size.height);
}

// Whether a point of the window lies on its content, where the guest is.
static bool
on_content(NSPoint where)
{
   return NSPointInRect([view convertPoint:where fromView:nil], view.bounds);
}

// Whether a point of the content lies near enough to its left, right or bottom edge that a press
// there may start a drag of the window's size. Above the content is the title bar; in full
// screen nothing is resized.
static bool
near_edge(NSPoint where)
{
   if (window.styleMask & NSWindowStyleMaskFullScreen)
      return false;
   NSPoint local = [view convertPoint:where fromView:nil];
   NSSize size = view.bounds.size;
   bool side = local.x < RESIZE_EDGE || local.x >= size.width - RESIZE_EDGE;
   bool corner = local.y < RESIZE_CORNER && (local.x < RESIZE_CORNER || local.x >= size.width - RESIZE_CORNER);
   return side || corner || local.y < RESIZE_EDGE;
}

// The smallest content the window is let shrink to, in points of its screen now.
static NSSize
smallest_points(void)
{
   return [view convertSizeFromBacking:NSMakeSize(SMALLEST_PIXELS, SMALLEST_PIXELS)];
}

// A content size in points held to the smallest.
static NSSize
at_least_smallest(NSSize points)
{
   NSSize least = smallest_points();
   return NSMakeSize(MAX(points.width, least.width), MAX(points.height, least.height));
}

// The guest gets the left press that was held back.
static void
hand_held_press(void)
{
   if (!press_held)
      return;
   press_held = false;
   callbacks.button(0, true);
}

// The device-dependent bit of a modifier key in an event's flags, from IOKit's IOLLEvent.h, or 0
// for a key that is no modifier of that kind.
static NSUInteger
modifier_bit(unsigned short code)
{
   switch (code) {
   case kVK_Control: return 0x1;
   case kVK_Shift: return 0x2;
   case kVK_RightShift: return 0x4;
   case kVK_Command: return 0x8;
   case kVK_RightCommand: return 0x10;
   case kVK_Option: return 0x20;
   case kVK_RightOption: return 0x40;
   case kVK_RightControl: return 0x2000;
   default: return 0;
   }
}

// A line for the process's output, for whoever reads how a person's session went. Only for what
// happens a few times a session: the window taking or losing the keyboard, a drag of its size.
static void
say(NSString *line)
{
   fprintf(stderr, "pvg-host: %s\n", line.UTF8String);
}

// A line with the window's content in points.
static void
say_points(NSString *what)
{
   NSSize points = view.bounds.size;
   say([NSString stringWithFormat:@"%@, %.0f by %.0f points", what, points.width, points.height]);
}

static void
release_all(void)
{
   wheel_right = wheel_up = 0;
   press_held = false;
   callbacks.released();
}

static void
scroll(NSEvent *event)
{
   double right = -event.scrollingDeltaX, up = event.scrollingDeltaY;
   if (event.hasPreciseScrollingDeltas) {
      right /= POINTS_A_STEP;
      up /= POINTS_A_STEP;
   }
   wheel_right += right;
   wheel_up += up;
   int32_t steps_right = (int32_t)trunc(wheel_right), steps_up = (int32_t)trunc(wheel_up);
   wheel_right -= steps_right;
   wheel_up -= steps_up;
   // A turn the other way starts afresh.
   if (event.phase == NSEventPhaseEnded || event.momentumPhase == NSEventPhaseEnded)
      wheel_right = wheel_up = 0;
   if (steps_right || steps_up)
      callbacks.wheel(steps_right, steps_up);
}

static uint32_t
button_of(NSEvent *event)
{
   switch (event.type) {
   case NSEventTypeLeftMouseDown:
   case NSEventTypeLeftMouseUp: return 0;
   case NSEventTypeRightMouseDown:
   case NSEventTypeRightMouseUp: return 1;
   default: return event.buttonNumber == 2 ? 2 : (uint32_t)MIN(event.buttonNumber, 4);
   }
}

// Hands an event of the window to the guest. Returns whether it was the guest's alone, so that
// AppKit doesn't act on it too.
static bool
take_event(NSEvent *event)
{
   MAIN_STEP("take_event");
   bool drag_part = made_up_drag && (event.type == NSEventTypeLeftMouseDragged || event.type == NSEventTypeLeftMouseUp);
   if (event.type == NSEventTypeLeftMouseUp)
      made_up_drag = false;
   if (input == PVG_INPUT_NONE || event.window != window || (input == PVG_INPUT_TEST && !made_up && !drag_part))
      return false;
   // The pointer and its buttons are the window's own while its size is dragged.
   if (live_resize && event.type != NSEventTypeKeyDown && event.type != NSEventTypeKeyUp &&
       event.type != NSEventTypeFlagsChanged)
      return false;
   switch (event.type) {
   case NSEventTypeKeyDown:
      // The guest repeats a held key itself.
      if (!event.ARepeat)
         callbacks.key(event.keyCode, true);
      return true;
   case NSEventTypeKeyUp:
      callbacks.key(event.keyCode, false);
      return true;
   case NSEventTypeFlagsChanged:
      if (event.keyCode == kVK_CapsLock) {
         // The Mac says when Caps Lock turns on and when it turns off, and a guest turns it over
         // with each press.
         callbacks.key(event.keyCode, true);
         callbacks.key(event.keyCode, false);
      } else if (modifier_bit(event.keyCode)) {
         callbacks.key(event.keyCode, (event.modifierFlags & modifier_bit(event.keyCode)) != 0);
      }
      return true;
   case NSEventTypeMouseMoved:
      // Over the title bar the pointer is the Mac's.
      if (on_content(event.locationInWindow))
         point_at(event.locationInWindow);
      return false;
   case NSEventTypeLeftMouseDragged:
   case NSEventTypeRightMouseDragged:
   case NSEventTypeOtherMouseDragged:
      // A press held back near the edge was no drag of the window's size once a move comes here
      // after the one that went far enough for AppKit to decide. A drag that leaves the window
      // holds the guest's pointer at the picture's edge.
      if (press_held && event.type == NSEventTypeLeftMouseDragged) {
         if (held_far)
            hand_held_press();
         else if (fabs(event.locationInWindow.x - held_at.x) >= START_DISTANCE ||
                  fabs(event.locationInWindow.y - held_at.y) >= START_DISTANCE)
            held_far = true;
      }
      point_at(event.locationInWindow);
      return false;
   case NSEventTypeLeftMouseDown:
   case NSEventTypeRightMouseDown:
   case NSEventTypeOtherMouseDown:
      hand_held_press();
      if (!on_content(event.locationInWindow))
         return false;
      point_at(event.locationInWindow);
      if (event.type == NSEventTypeLeftMouseDown && near_edge(event.locationInWindow)) {
         press_held = true;
         held_at = event.locationInWindow;
         held_far = false;
      } else
         callbacks.button(button_of(event), true);
      return false;
   case NSEventTypeLeftMouseUp:
   case NSEventTypeRightMouseUp:
   case NSEventTypeOtherMouseUp:
      // A button let go anywhere is let go in the guest; one it never had down is dropped there.
      // A press held back whose drag went far enough is the guest's; one that did not is a click
      // on the edge, and the guest gets neither it nor its release.
      if (press_held && event.type == NSEventTypeLeftMouseUp && !held_far) {
         press_held = false;
         return false;
      }
      hand_held_press();
      callbacks.button(button_of(event), false);
      return false;
   case NSEventTypeScrollWheel:
      if (on_content(event.locationInWindow))
         scroll(event);
      return true;
   default:
      return false;
   }
}

@implementation PVGView
- (BOOL)acceptsFirstResponder
{
   MAIN_STEP("acceptsFirstResponder");
   return input != PVG_INPUT_NONE;
}
- (BOOL)acceptsFirstMouse:(NSEvent *)event
{
   MAIN_STEP("acceptsFirstMouse");
   return input != PVG_INPUT_NONE;
}
- (void)updateTrackingAreas
{
   MAIN_STEP("updateTrackingAreas");
   for (NSTrackingArea *area in self.trackingAreas)
      [self removeTrackingArea:area];
   if (input != PVG_INPUT_NONE) {
      NSTrackingAreaOptions options = NSTrackingMouseEnteredAndExited | NSTrackingMouseMoved |
                                      NSTrackingCursorUpdate | NSTrackingActiveInKeyWindow |
                                      NSTrackingInVisibleRect;
      [self addTrackingArea:[[NSTrackingArea alloc] initWithRect:NSZeroRect options:options owner:self userInfo:nil]];
   }
   [super updateTrackingAreas];
}
- (void)mouseEntered:(NSEvent *)event
{
   MAIN_STEP("mouseEntered");
   pointer_inside = true;
   place_cursor();
}
- (void)mouseExited:(NSEvent *)event
{
   MAIN_STEP("mouseExited");
   pointer_inside = false;
   place_cursor();
   [NSCursor.arrowCursor set];
}
- (void)cursorUpdate:(NSEvent *)event
{
   MAIN_STEP("cursorUpdate");
   [pointer_now() set];
}
// Keys come through the monitor; these keep AppKit from beeping at any it hands on.
- (void)keyDown:(NSEvent *)event
{
   MAIN_STEP("keyDown");
}
- (void)keyUp:(NSEvent *)event
{
   MAIN_STEP("keyUp");
}
@end

@implementation PVGWindowDelegate
// A drag of the window's edge starts with a button pressed on it, which the monitor has seen and,
// where the edge lies over the picture, handed to the guest; AppKit's loop for the drag takes its
// release. So the guest lets go of everything when the drag starts and again when it ends.
- (void)windowWillStartLiveResize:(NSNotification *)notification
{
   MAIN_STEP("windowWillStartLiveResize");
   live_resize = true;
   if (input != PVG_INPUT_NONE)
      say([NSString stringWithFormat:@"a drag of the window's size starts, %s, and the guest lets go of every key and button",
                                     press_held ? "the press that started it held back from the guest"
                                                : "with no press held back"]);
   release_all();
}
- (void)windowDidEndLiveResize:(NSNotification *)notification
{
   MAIN_STEP("windowDidEndLiveResize");
   live_resize = false;
   // A test's drag that AppKit took for a drag of the size has had its release.
   made_up_drag = false;
   release_all();
   if (input != PVG_INPUT_NONE) {
      NSSize points = view.bounds.size;
      say([NSString stringWithFormat:@"the drag of the window's size ends at %.0f by %.0f points, %s",
                                     points.width, points.height,
                                     window.keyWindow ? "the window has the keyboard" : "the keyboard is elsewhere"]);
   }
   place_cursor();
   tell_size();
   // The picture lies elsewhere in the window now: the guest's pointer goes where the Mac's is.
   NSPoint where = window.mouseLocationOutsideOfEventStream;
   if (input == PVG_INPUT_FOCUS && window.keyWindow && on_content(where))
      point_at(where);
}
// Every size the window passes through, also in a drag; the thread that serves the guest waits
// until the size stays put.
- (void)windowDidResize:(NSNotification *)notification
{
   MAIN_STEP("windowDidResize");
   place_cursor();
   tell_size();
}
// A screen of another scale, as when the window is moved to another screen. The window keeps
// its content's pixels, and so the guest its mode and its desktop's scale: weston reads its scale
// once, at its start. The window then shows larger or smaller on the other screen, one to one.
- (void)windowDidChangeBackingProperties:(NSNotification *)notification
{
   MAIN_STEP("windowDidChangeBackingProperties");
   CGFloat before = [notification.userInfo[NSBackingPropertyOldScaleFactorKey] doubleValue];
   CGFloat now = window.backingScaleFactor;
   window.contentMinSize = smallest_points();
   if (before > 0 && now > 0 && before != now && !(window.styleMask & NSWindowStyleMaskFullScreen)) {
      NSSize points = view.bounds.size;
      NSSize kept = NSMakeSize(round(points.width * before / now), round(points.height * before / now));
      if (pixels_kept.width > 0 && pixels_kept.height > 0)
         kept = NSMakeSize(round(pixels_kept.width / now), round(pixels_kept.height / now));
      if (input != PVG_INPUT_NONE)
         say([NSString stringWithFormat:@"the window is on a screen of scale %.0f, was %.0f: %.0f by %.0f points to keep its pixels",
                                        now, before, kept.width, kept.height]);
      [window setContentSize:at_least_smallest(kept)];
   }
   place_cursor();
   tell_size();
}
- (void)windowDidChangeScreen:(NSNotification *)notification
{
   MAIN_STEP("windowDidChangeScreen");
   tell_size();
}
// A change to full screen or back is a few in a session, and says when it starts and ends.
- (void)windowWillEnterFullScreen:(NSNotification *)notification
{
   MAIN_STEP("windowWillEnterFullScreen");
   say(@"the window goes to full screen");
}
- (void)windowDidEnterFullScreen:(NSNotification *)notification
{
   MAIN_STEP("windowDidEnterFullScreen");
   say_points(@"the window is in full screen");
   place_cursor();
   tell_size();
}
- (void)windowDidFailToEnterFullScreen:(NSWindow *)sender
{
   MAIN_STEP("windowDidFailToEnterFullScreen");
   say_points(@"the window did not go to full screen");
   place_cursor();
   tell_size();
}
- (void)windowWillExitFullScreen:(NSNotification *)notification
{
   MAIN_STEP("windowWillExitFullScreen");
   say(@"the window leaves full screen");
}
- (void)windowDidExitFullScreen:(NSNotification *)notification
{
   MAIN_STEP("windowDidExitFullScreen");
   say_points(@"the window has left full screen");
   place_cursor();
   tell_size();
}
- (void)windowDidFailToExitFullScreen:(NSWindow *)sender
{
   MAIN_STEP("windowDidFailToExitFullScreen");
   say_points(@"the window did not leave full screen");
   place_cursor();
   tell_size();
}
- (void)windowDidChangeOcclusionState:(NSNotification *)notification
{
   MAIN_STEP("windowDidChangeOcclusionState");
   keep_state();
}
- (void)windowDidBecomeKey:(NSNotification *)notification
{
   MAIN_STEP("windowDidBecomeKey");
   if (input == PVG_INPUT_FOCUS)
      say(@"the window has the keyboard");
}
- (void)windowDidResignKey:(NSNotification *)notification
{
   MAIN_STEP("windowDidResignKey");
   if (input == PVG_INPUT_FOCUS)
      say(@"the window has lost the keyboard, and the guest lets go of every key and button");
   release_all();
}
// Closing the window presses the guest's power button, with input or without, and the window
// stays until QEMU has gone. A window with neither input nor QEMU's monitor can't ask: it hides
// the picture and leaves the guest running.
- (BOOL)windowShouldClose:(NSWindow *)sender
{
   MAIN_STEP("windowShouldClose");
   if (!callbacks.closing() && input == PVG_INPUT_NONE)
      [sender orderOut:nil];
   return NO;
}
@end

// The screen the window opens on, by PVG_WINDOW_SCREEN: "builtin", the screen the system calls
// built in (the main one when there is none, as with the lid closed), "main", the one with the
// menu bar, or a number, the index in NSScreen.screens. Unset, the main one. With telling, a line
// says which.
static NSScreen *
window_screen(bool telling)
{
   NSArray<NSScreen *> *screens = NSScreen.screens;
   NSScreen *main = screens.firstObject;
   const char *word = getenv("PVG_WINDOW_SCREEN");
   if (!word || !*word || !main)
      return main;
   NSScreen *chosen = nil;
   NSString *why = @"";
   if (!strcmp(word, "builtin")) {
      for (NSScreen *screen in screens)
         if (CGDisplayIsBuiltin([screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue]))
            chosen = screen;
      if (!chosen)
         why = @", since there is no built-in screen";
   } else if (!strcmp(word, "main")) {
      chosen = main;
   } else {
      char *end;
      long index = strtol(word, &end, 10);
      if (*end == 0 && index >= 0 && index < (long)screens.count)
         chosen = screens[index];
      else
         why = [NSString stringWithFormat:@", since there is no screen %s of %lu", word,
                                          (unsigned long)screens.count];
   }
   if (!chosen)
      chosen = main;
   if (telling)
      say([NSString stringWithFormat:@"PVG_WINDOW_SCREEN=%s: the window opens on screen %lu, %@, of scale %.0f%@",
                                     word, (unsigned long)[screens indexOfObject:chosen], chosen.localizedName,
                                     chosen.backingScaleFactor, why]);
   return chosen;
}

// Whether PVG_WINDOW_FOCUS=no keeps the window from taking the keyboard and the app from being
// activated when the window opens, for a run with no person.
static bool
window_no_focus(void)
{
   const char *word = getenv("PVG_WINDOW_FOCUS");
   return word && !strcmp(word, "no");
}

// The window's frame for a content of this many points, centered in the screen's visible frame.
static NSRect
window_centered(NSScreen *screen, uint32_t width, uint32_t height, NSUInteger style)
{
   NSRect frame = [NSWindow frameRectForContentRect:NSMakeRect(0, 0, width, height) styleMask:style];
   NSRect room = screen.visibleFrame;
   frame.origin.x = round(NSMidX(room) - frame.size.width / 2);
   frame.origin.y = round(NSMidY(room) - frame.size.height / 2);
   return frame;
}

// A window that is not to take the keyboard says when its app was made active all the same, and
// whether a mouse button was down then, as when a person clicks the window, and where it was.
static void
watch_activation(void)
{
   [[NSNotificationCenter defaultCenter]
      addObserverForName:NSApplicationDidBecomeActiveNotification
                  object:nil
                   queue:nil
              usingBlock:^(__unused NSNotification *note) {
                 NSRect frame = window.frame;
                 say([NSString stringWithFormat:@"the app was made active, mouse buttons down 0x%lx, the window at "
                                                @"%.0f,%.0f %.0fx%.0f",
                                                (unsigned long)NSEvent.pressedMouseButtons, frame.origin.x,
                                                frame.origin.y, frame.size.width, frame.size.height]);
              }];
}

// A window that is on the screen before the app has finished launching makes AppKit activate the
// app then, when the app that started this process is in front (seen on macOS 27.2.1). So a window
// that is not to take the keyboard comes up once the launch is over, in front of nothing.
static void
order_front_after_launch(void)
{
   __block id observer = [[NSNotificationCenter defaultCenter]
      addObserverForName:NSApplicationDidFinishLaunchingNotification
                  object:nil
                   queue:nil
              usingBlock:^(__unused NSNotification *note) {
                 [[NSNotificationCenter defaultCenter] removeObserver:observer];
                 [window orderFront:nil];
              }];
}

// The backing scale of the screen the window opens on: 2 on a Retina screen, 1 on most others.
double
pvg_window_screen_scale(void)
{
   @autoreleasepool {
      NSScreen *screen = window_screen(false);
      return screen ? screen.backingScaleFactor : 1;
   }
}

// Whether the Mac's keyboard is an ISO one, whose key beside the left Shift and key left of 1
// the Mac names the other way round from the key's place.
bool
pvg_window_iso_keyboard(void)
{
   return KBGetLayoutType(LMGetKbdType()) == kKeyboardISO;
}

// Opens the window, with a content of this many points. With no input, or with input for tests,
// it opens in front of nothing and does not take the keyboard from whatever the user is doing.
bool
pvg_window_open(const char *title, uint32_t width, uint32_t height, const pvg_window_callbacks *given,
                int taking)
{
   @autoreleasepool {
      callbacks = *given;
      input = taking;
      bool focus = input == PVG_INPUT_FOCUS && !window_no_focus();
      [NSApplication sharedApplication];
      // A test's window may never be made active: AppKit activates a process whose window takes
      // a press, made up or not, unless it is Prohibited (seen 2026-11-27), and a process with a
      // window may be activated at its start. So a test never takes the user's keyboard.
      [NSApp setActivationPolicy:focus                    ? NSApplicationActivationPolicyRegular
                                 : input == PVG_INPUT_TEST ? NSApplicationActivationPolicyProhibited
                                                           : NSApplicationActivationPolicyAccessory];
      NSUInteger style = NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskResizable |
                         NSWindowStyleMaskMiniaturizable;
      NSScreen *screen = window_screen(true);
      // Made on the screen, the window has that screen's backing scale from the start, and so the
      // guest's first mode is the window's points times that scale. The frame is in the screens'
      // common coordinates, the content's rectangle at the start from the screen's own origin.
      NSRect frame = screen ? window_centered(screen, width, height, style) : NSMakeRect(80, 80, width, height);
      NSRect content = [NSWindow contentRectForFrameRect:frame styleMask:style];
      if (screen)
         content = NSOffsetRect(content, -screen.frame.origin.x, -screen.frame.origin.y);
      window = [[NSWindow alloc] initWithContentRect:content
                                           styleMask:style
                                             backing:NSBackingStoreBuffered
                                               defer:NO
                                              screen:screen];
      if (!window)
         return false;
      [window setFrame:frame display:NO];
      window.title = @(title);
      window.releasedWhenClosed = NO;
      window.collectionBehavior |= NSWindowCollectionBehaviorFullScreenPrimary;
      delegate = [PVGWindowDelegate new];
      window.delegate = delegate;

      view = [[PVGView alloc] initWithFrame:NSMakeRect(0, 0, width, height)];
      window.contentView = view;
      // No smaller than the guest's smallest mode, so that the picture stays one to one.
      window.contentMinSize = smallest_points();
      CALayer *back = [CALayer layer];
      back.backgroundColor = CGColorGetConstantColor(kCGColorBlack);
      view.layer = back;
      view.wantsLayer = YES;
      picture = [CAMetalLayer layer];
      picture.frame = back.bounds;
      picture.autoresizingMask = kCALayerWidthSizable | kCALayerHeightSizable;
      picture.contentsGravity = kCAGravityResizeAspect;
      [back addSublayer:picture];
      cursor = [CALayer layer];
      cursor.hidden = YES;
      cursor.magnificationFilter = kCAFilterNearest;
      [back addSublayer:cursor];

      if (input != PVG_INPUT_NONE) {
         NSImage *nothing = [[NSImage alloc] initWithSize:NSMakeSize(1, 1)];
         no_pointer = [[NSCursor alloc] initWithImage:nothing hotSpot:NSZeroPoint];
         window.acceptsMouseMovedEvents = YES;
         NSEventMask mask = NSEventMaskKeyDown | NSEventMaskKeyUp | NSEventMaskFlagsChanged |
                            NSEventMaskMouseMoved | NSEventMaskLeftMouseDown | NSEventMaskLeftMouseUp |
                            NSEventMaskRightMouseDown | NSEventMaskRightMouseUp | NSEventMaskOtherMouseDown |
                            NSEventMaskOtherMouseUp | NSEventMaskLeftMouseDragged |
                            NSEventMaskRightMouseDragged | NSEventMaskOtherMouseDragged | NSEventMaskScrollWheel;
         monitor = [NSEvent addLocalMonitorForEventsMatchingMask:mask
                                                         handler:^NSEvent *(NSEvent *event) {
                                                            return take_event(event) ? nil : event;
                                                         }];
         [[NSNotificationCenter defaultCenter] addObserverForName:NSApplicationDidResignActiveNotification
                                                           object:nil
                                                            queue:nil
                                                       usingBlock:^(__unused NSNotification *note) {
                                                          release_all();
                                                       }];
      }
      if (!focus)
         watch_activation();
      if (focus) {
         [window makeKeyAndOrderFront:nil];
         [window makeFirstResponder:view];
         [NSApp activate];
      } else {
         if (input == PVG_INPUT_FOCUS)
            [window makeFirstResponder:view];
         order_front_after_launch();
      }
      // The guest's first mode list is the window's own size.
      tell_size();
      return true;
   }
}

// The layer the guest's frames are drawn into.
void *
pvg_window_layer(void)
{
   return (__bridge void *)picture;
}

// Serves the window until pvg_window_stop.
void
pvg_window_run(void)
{
   @autoreleasepool {
      [NSApp run];
   }
}

void
pvg_window_stop(void)
{
   dispatch_async(dispatch_get_main_queue(), ^{
      MAIN_STEP("pvg_window_stop");
      [window orderOut:nil];
      [NSApp stop:nil];
      // The run loop looks at its stop flag only when an event has come.
      NSEvent *event = [NSEvent otherEventWithType:NSEventTypeApplicationDefined
                                          location:NSZeroPoint
                                     modifierFlags:0
                                         timestamp:0
                                      windowNumber:0
                                           context:nil
                                           subtype:0
                                             data1:0
                                             data2:0];
      [NSApp postEvent:event atStart:YES];
   });
}

// The window's content in pixels of the screen, whether the window is on a screen and not wholly
// covered, and whether the cursor's layer shows, as the first thread last saw them. For whoever
// asks how the window is doing, from any thread, without waiting for the first thread: a thread
// that waited for it once stood still with the guest behind it.
void
pvg_window_state(uint32_t *width, uint32_t *height, bool *visible, bool *cursor_showing)
{
   *width = atomic_load(&kept_width);
   *height = atomic_load(&kept_height);
   *visible = atomic_load(&kept_visible);
   *cursor_showing = atomic_load(&kept_cursor);
}

// Gives the window's content this size in pixels of the screen, as a user's drag would.
void
pvg_window_resize(uint32_t width, uint32_t height)
{
   dispatch_async(dispatch_get_main_queue(), ^{
      MAIN_STEP("pvg_window_resize");
      NSSize points = [view convertSizeFromBacking:NSMakeSize(width, height)];
      [window setContentSize:at_least_smallest(points)];
      place_cursor();
      tell_size();
   });
}

// What the window's close button does, in any mode. Asked of the delegate as AppKit asks it, not
// by performClose:, which beeps when the window stays.
void
pvg_window_close(void)
{
   dispatch_async(dispatch_get_main_queue(), ^{
      MAIN_STEP("pvg_window_close");
      [delegate windowShouldClose:window];
   });
}

// Takes the window to full screen or back, as its green button does.
void
pvg_window_full_screen(void)
{
   dispatch_async(dispatch_get_main_queue(), ^{
      MAIN_STEP("pvg_window_full_screen");
      [window toggleFullScreen:nil];
   });
}

// The guest's picture has this size from now on.
void
pvg_window_picture_size(uint32_t width, uint32_t height)
{
   dispatch_async(dispatch_get_main_queue(), ^{
      MAIN_STEP("pvg_window_picture_size");
      picture_size = CGSizeMake(width, height);
      place_cursor();
   });
}

// A new picture for the cursor: four bytes a pixel, blue first, rows from the top.
void
pvg_window_glyph(const uint8_t *pixels, uint32_t width, uint32_t height, uint32_t hot_x, uint32_t hot_y)
{
   CGColorSpaceRef space = CGColorSpaceCreateDeviceRGB();
   CGContextRef context = CGBitmapContextCreate(NULL, width, height, 8, (size_t)width * 4, space,
                                                kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little);
   CGColorSpaceRelease(space);
   if (!context)
      return;
   memcpy(CGBitmapContextGetData(context), pixels, (size_t)width * height * 4);
   CGImageRef image = CGBitmapContextCreateImage(context);
   CGContextRelease(context);
   dispatch_async(dispatch_get_main_queue(), ^{
      MAIN_STEP("pvg_window_glyph");
      cursor.contents = (__bridge id)image;
      if (cursor_image)
         CGImageRelease(cursor_image);
      cursor_image = image;
      guest_pointer = nil;
      cursor_size = CGSizeMake(width, height);
      cursor_hot = CGPointMake(hot_x, hot_y);
      place_cursor();
   });
}

// Where the guest's cursor is, in the picture's pixels, and whether it shows.
void
pvg_window_cursor(int32_t x, int32_t y, bool shown)
{
   dispatch_async(dispatch_get_main_queue(), ^{
      MAIN_STEP("pvg_window_cursor");
      cursor_place = CGPointMake(x, y);
      cursor_shown = shown;
      place_cursor();
   });
}

// Hands a test's event to the window's input, on the first thread.
static void
test_event(int kind, int64_t a, int64_t b, double x, double y)
{
   NSPoint where = [view convertPoint:NSMakePoint(x, view.bounds.size.height - y) toView:nil];
   NSInteger number = window.windowNumber;
   NSTimeInterval now = NSProcessInfo.processInfo.systemUptime;
   NSEvent *event = nil;
   switch (kind) {
   case PVG_TEST_KEY:
      event = [NSEvent keyEventWithType:b ? NSEventTypeKeyDown : NSEventTypeKeyUp
                               location:NSZeroPoint
                          modifierFlags:0
                              timestamp:now
                           windowNumber:number
                                context:nil
                             characters:@""
            charactersIgnoringModifiers:@""
                              isARepeat:NO
                                keyCode:(unsigned short)a];
      break;
   case PVG_TEST_FLAGS:
      event = [NSEvent keyEventWithType:NSEventTypeFlagsChanged
                               location:NSZeroPoint
                          modifierFlags:(NSEventModifierFlags)b
                              timestamp:now
                           windowNumber:number
                                context:nil
                             characters:@""
            charactersIgnoringModifiers:@""
                              isARepeat:NO
                                keyCode:(unsigned short)a];
      break;
   case PVG_TEST_MOVE:
      event = [NSEvent mouseEventWithType:NSEventTypeMouseMoved
                                 location:where
                            modifierFlags:0
                                timestamp:now
                             windowNumber:number
                                  context:nil
                              eventNumber:0
                               clickCount:0
                                 pressure:0];
      break;
   case PVG_TEST_BUTTON: {
      static const NSEventType downs[] = {NSEventTypeLeftMouseDown, NSEventTypeRightMouseDown,
                                          NSEventTypeOtherMouseDown};
      static const NSEventType ups[] = {NSEventTypeLeftMouseUp, NSEventTypeRightMouseUp,
                                        NSEventTypeOtherMouseUp};
      if (a < 0 || a > 2)
         return;
      event = [NSEvent mouseEventWithType:b ? downs[a] : ups[a]
                                 location:where
                            modifierFlags:0
                                timestamp:now
                             windowNumber:number
                                  context:nil
                              eventNumber:0
                               clickCount:1
                                 pressure:b ? 1 : 0];
      break;
   }
   case PVG_TEST_SCROLL: {
      // Made as a CGEvent, which is the only way to make a scroll event, and never posted.
      // The second wheel's lines count to the left.
      CGEventRef made = CGEventCreateScrollWheelEvent2(NULL, kCGScrollEventUnitLine, 2, (int32_t)b,
                                                       (int32_t)-a, 0);
      if (!made)
         return;
      CGEventSetLocation(made, NSPointToCGPoint([window convertPointToScreen:where]));
      event = [NSEvent eventWithCGEvent:made];
      CFRelease(made);
      // It knows no window, so it is scrolled where it was made for.
      if (event && on_content(where))
         scroll(event);
      return;
   }
   case PVG_TEST_RESIGN:
      [delegate windowDidResignKey:[NSNotification notificationWithName:NSWindowDidResignKeyNotification
                                                                object:window]];
      return;
   case PVG_TEST_CLOSE:
      [delegate windowShouldClose:window];
      return;
   case PVG_TEST_EDGE: {
      NSEvent *press = [NSEvent mouseEventWithType:NSEventTypeLeftMouseDown
                                          location:where
                                     modifierFlags:0
                                         timestamp:now
                                      windowNumber:number
                                           context:nil
                                       eventNumber:0
                                        clickCount:1
                                          pressure:1];
      take_event(press);
      [delegate windowWillStartLiveResize:[NSNotification
                                              notificationWithName:NSWindowWillStartLiveResizeNotification
                                                            object:window]];
      [window setContentSize:at_least_smallest([view convertSizeFromBacking:NSMakeSize(a, b)])];
      [delegate windowDidEndLiveResize:[NSNotification
                                           notificationWithName:NSWindowDidEndLiveResizeNotification
                                                         object:window]];
      return;
   }
   case PVG_TEST_DRAG: {
      // AppKit's own: the press goes to NSApp as a person's would, through the monitor to the
      // window, and the drag's three moves and the release wait in the process's queue. The
      // window either starts its loop for a drag of its size, which takes them from the queue,
      // or hands the press on as a click, and they come through the monitor after it.
      NSEvent *events[5];
      for (int i = 0; i < 5; i++) {
         double part = MIN(i, 3) / 3.0;
         NSPoint at = [view convertPoint:NSMakePoint(x + a * part, view.bounds.size.height - (y + b * part))
                                  toView:nil];
         NSEventType type = i == 0 ? NSEventTypeLeftMouseDown
                          : i == 4 ? NSEventTypeLeftMouseUp
                                   : NSEventTypeLeftMouseDragged;
         events[i] = [NSEvent mouseEventWithType:type
                                        location:at
                                   modifierFlags:0
                                       timestamp:now + 0.01 * i
                                    windowNumber:number
                                         context:nil
                                     eventNumber:0
                                      clickCount:1
                                        pressure:i == 4 ? 0 : 1];
      }
      // AppKit activates a process whose window takes a press, made-up or not, unless it may never
      // be activated (seen 2026-11-27): a test's press must not take the keyboard from the user.
      if (NSApp.activationPolicy != NSApplicationActivationPolicyProhibited)
         [NSApp setActivationPolicy:NSApplicationActivationPolicyProhibited];
      for (int i = 1; i < 5; i++)
         [NSApp postEvent:events[i] atStart:NO];
      made_up_drag = true;
      say([NSString stringWithFormat:@"test: a press at %.0f, %.0f and a drag by %lld, %lld, handed to AppKit", x, y, a, b]);
      [NSApp sendEvent:events[0]];
      return;
   }
   case PVG_TEST_SCREEN: {
      // The window's frame centered on another screen, as a person's move of it there.
      NSArray<NSScreen *> *screens = NSScreen.screens;
      if (a < 0 || a >= (int64_t)screens.count)
         return;
      NSRect room = screens[a].visibleFrame, frame = window.frame;
      frame.origin.x = round(NSMidX(room) - frame.size.width / 2);
      frame.origin.y = round(NSMidY(room) - frame.size.height / 2);
      say([NSString stringWithFormat:@"test: the window to screen %lld, %@, of scale %.0f", a,
                                     screens[a].localizedName, screens[a].backingScaleFactor]);
      [window setFrame:frame display:YES];
      return;
   }
   default:
      return;
   }
   if (event)
      take_event(event);
}

// An event made up for a test, handed to the window's input as one of AppKit's would be. Points
// are the window content's, from its top left. Only with PVG_INPUT_TEST.
//   PVG_TEST_KEY     a: the Mac's key code, b: 1 down or 0 up
//   PVG_TEST_FLAGS   a: the key code of a modifier, b: the event's modifier flags
//   PVG_TEST_MOVE    x, y
//   PVG_TEST_BUTTON  a: 0 left, 1 right, 2 middle, b: 1 down or 0 up, at x, y
//   PVG_TEST_SCROLL  a: lines to the right, b: lines up, at x, y
//   PVG_TEST_RESIGN  the window stops being the key window
//   PVG_TEST_CLOSE   the window's close button
//   PVG_TEST_EDGE    a drag of the window's edge by a person, as AppKit hands it over: the
//                    press at x, y through the monitor, then AppKit's start of a live resize,
//                    the window's content at a by b pixels, and the end, with the drag's moves
//                    and the release kept from the monitor as AppKit's loop keeps them
//   PVG_TEST_DRAG    a left press at x, y, a drag by a, b points and the release, handed to
//                    AppKit itself, which decides whether it is a drag of the window's size
//   PVG_TEST_SCREEN  the window centered on screen a of NSScreen.screens
void
pvg_window_test_event(int kind, int64_t a, int64_t b, double x, double y)
{
   dispatch_async(dispatch_get_main_queue(), ^{
      MAIN_STEP("pvg_window_test_event");
      if (input != PVG_INPUT_TEST)
         return;
      made_up = true;
      test_event(kind, a, b, x, y);
      made_up = false;
   });
}


static uint64_t
now_nanoseconds(void)
{
   return clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
}

// Asks the first thread to note that it has run, as soon as it runs what was handed to it.
void
pvg_window_ping(void)
{
   uint64_t first = 0;
   atomic_compare_exchange_strong(&main_answered, &first, now_nanoseconds());
   dispatch_async(dispatch_get_main_queue(), ^{
      atomic_store(&main_answered, now_nanoseconds());
   });
}

// How the first thread is doing, from any thread: how many milliseconds ago it last ran the work
// of pvg_window_ping, the function of this file it is in ("" for none) and the one it last left,
// and its run loop's mode now, as far as room goes.
void
pvg_window_look(uint64_t *answered_ago, const char **inside, const char **left, char *mode, size_t room)
{
   uint64_t answered = atomic_load(&main_answered);
   uint64_t now = now_nanoseconds();
   *answered_ago = answered && now > answered ? (now - answered) / 1000000 : 0;
   const char *in = atomic_load(&main_in), *out = atomic_load(&main_left);
   *inside = in ? in : "";
   *left = out ? out : "";
   if (!room)
      return;
   snprintf(mode, room, "none");
   CFStringRef current = CFRunLoopCopyCurrentMode(CFRunLoopGetMain());
   if (current) {
      CFStringGetCString(current, mode, (CFIndex)room, kCFStringEncodingUTF8);
      CFRelease(current);
   }
}
