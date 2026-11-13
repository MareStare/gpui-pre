//! Pen input from the valuators of XInput 2.
//!
//! A pen is a slave pointer with an `Abs Pressure` valuator. The master
//! pointer gives its events with the id of the pen in `sourceid` and the
//! pressure among the valuators of the event. Qt reads pens the same way in
//! `QXcbConnection::xi2ReportTabletEvent`:
//! <https://code.qt.io/cgit/qt/qtbase.git/tree/src/plugins/platforms/xcb/qxcbconnection_xi2.cpp>.
//!
//! XInput 2 has no proximity events. A pen is near from its first event
//! until another device moves the pointer or the pointer leaves the window.

use super::{LEFT_BUTTON_BIT, fp3232_to_f32, get_valuator_axis_index};
use gpui::{PenEvent, Pixels, Point};
use std::collections::BTreeMap;
use x11rb::protocol::xinput;

/// The pens among the pointer devices.
#[derive(Debug, Default)]
pub(super) struct Pens {
    devices: BTreeMap<xinput::DeviceId, PenDevice>,
    near: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PenDevice {
    valuator: u16,
    min: f32,
    max: f32,

    /// The last pressure from `0.0` to `1.0`. An event has the valuator only
    /// when its value changed.
    pressure: f32,
}

/// A pointer event of the master pointer.
pub(super) struct PointerSample<'a> {
    pub source: xinput::DeviceId,

    pub position: Point<Pixels>,

    /// The tip of the pen or the left mouse button is down.
    pub touches: bool,
    pub valuator_mask: &'a [u32],
    pub axisvalues: &'a [xinput::Fp3232],
}

/// The kind of a pointer event of XInput 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PointerAction {
    Press,
    Release,
    Motion,
}

impl<'a> PointerSample<'a> {
    /// The sample of `event`. The button mask of an event holds the buttons
    /// from before the event. Button 1 is the tip of a pen.
    pub(super) fn new(
        event: &'a xinput::ButtonPressEvent,
        action: PointerAction,
        position: Point<Pixels>,
    ) -> Self {
        let tip = event.detail == 1;
        let down = event
            .button_mask
            .first()
            .is_some_and(|mask| mask & LEFT_BUTTON_BIT != 0);
        let touches = match action {
            PointerAction::Press => tip || down,
            PointerAction::Release => !tip && down,
            PointerAction::Motion => down,
        };
        Self {
            source: event.sourceid,
            position,
            touches,
            valuator_mask: &event.valuator_mask,
            axisvalues: &event.axisvalues,
        }
    }
}

impl Pens {
    /// Finds the pens among `devices`. `pressure_label` is the
    /// `Abs Pressure` atom.
    pub(super) fn set_devices(&mut self, devices: &[xinput::XIDeviceInfo], pressure_label: u32) {
        self.devices = devices
            .iter()
            .filter(|info| info.type_ == xinput::DeviceType::SLAVE_POINTER)
            .filter_map(|info| {
                let valuator = info
                    .classes
                    .iter()
                    .filter_map(|class| class.data.as_valuator())
                    .find(|valuator| valuator.label == pressure_label)?;
                let pen = PenDevice {
                    valuator: valuator.number,
                    min: fp3232_to_f32(valuator.min),
                    max: fp3232_to_f32(valuator.max),
                    pressure: 0.0,
                };
                Some((info.deviceid, pen))
            })
            .collect();
    }

    /// The pen event of a pointer event.
    pub(super) fn sample(&mut self, sample: PointerSample) -> Option<PenEvent> {
        let Some(pen) = self.devices.get_mut(&sample.source) else {
            return self.leave();
        };
        self.near = true;
        if let Some(value) = get_valuator_axis_index(sample.valuator_mask, pen.valuator)
            .and_then(|index| sample.axisvalues.get(index))
        {
            let range = pen.max - pen.min;
            if range > 0.0 {
                pen.pressure = ((fp3232_to_f32(*value) - pen.min) / range).clamp(0.0, 1.0);
            }
        }
        Some(if sample.touches {
            PenEvent::Touch {
                position: sample.position,
                pressure: pen.pressure,
            }
        } else {
            PenEvent::Hover
        })
    }

    /// The pointer left the window. Gives `Leave` once for a pen that was
    /// near.
    pub(super) fn leave(&mut self) -> Option<PenEvent> {
        std::mem::take(&mut self.near).then_some(PenEvent::Leave)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px};

    const PEN: xinput::DeviceId = 12;
    const MOUSE: xinput::DeviceId = 9;

    fn fp(value: i32) -> xinput::Fp3232 {
        xinput::Fp3232 {
            integral: value,
            frac: 0,
        }
    }

    fn pens() -> Pens {
        let pen = PenDevice {
            valuator: 2,
            min: 0.0,
            max: 2048.0,
            pressure: 0.0,
        };
        Pens {
            devices: BTreeMap::from([(PEN, pen)]),
            near: false,
        }
    }

    fn sample<'a>(
        source: xinput::DeviceId,
        touches: bool,
        valuator_mask: &'a [u32],
        axisvalues: &'a [xinput::Fp3232],
    ) -> PointerSample<'a> {
        PointerSample {
            source,
            position: point(px(10.5), px(-3.25)),
            touches,
            valuator_mask,
            axisvalues,
        }
    }

    #[test]
    fn a_pen_reads_its_pressure_valuator_and_the_mouse_leaves_it() {
        let mut pens = pens();
        assert_eq!(pens.sample(sample(MOUSE, true, &[], &[])), None);

        let x_y_pressure = [fp(100), fp(200), fp(512)];
        assert_eq!(
            pens.sample(sample(PEN, false, &[0b111], &x_y_pressure)),
            Some(PenEvent::Hover)
        );
        let touch = |pressure| {
            Some(PenEvent::Touch {
                position: point(px(10.5), px(-3.25)),
                pressure,
            })
        };
        assert_eq!(
            pens.sample(sample(PEN, true, &[0b111], &x_y_pressure)),
            touch(0.25)
        );
        // An event with no pressure valuator keeps the last pressure.
        let x = [fp(101)];
        assert_eq!(pens.sample(sample(PEN, true, &[0b1], &x)), touch(0.25));
        assert_eq!(
            pens.sample(sample(PEN, true, &[0b100], &[fp(4096)])),
            touch(1.0)
        );

        assert_eq!(
            pens.sample(sample(MOUSE, false, &[], &[])),
            Some(PenEvent::Leave)
        );
        assert_eq!(pens.sample(sample(MOUSE, false, &[], &[])), None);
        assert_eq!(pens.leave(), None);
    }
}
