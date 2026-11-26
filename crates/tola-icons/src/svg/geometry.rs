//! SVG user-space view box and absolute length parsing.

use std::str::FromStr;

use super::InvalidSvg;

const CSS_PIXELS_PER_INCH: f64 = 96.0;

/// Validated SVG view box in user-space coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewBox {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

impl ViewBox {
    pub(crate) fn new(left: f64, top: f64, width: f64, height: f64) -> Result<Self, InvalidSvg> {
        if ![left, top, width, height, left + width, top + height]
            .iter()
            .all(|n| n.is_finite())
            || width <= 0.0
            || height <= 0.0
            || !(width / height).is_finite()
            || width / height <= 0.0
            || !(height / width).is_finite()
            || height / width <= 0.0
        {
            return Err(InvalidSvg::Geometry);
        }
        Ok(Self {
            left,
            top,
            width,
            height,
        })
    }

    /// Return the minimum x coordinate of the view box.
    pub fn left(self) -> f64 {
        self.left
    }

    /// Return the minimum y coordinate of the view box.
    pub fn top(self) -> f64 {
        self.top
    }

    /// Return the positive finite user-space width.
    pub fn width(self) -> f64 {
        self.width
    }

    /// Return the positive finite user-space height.
    pub fn height(self) -> f64 {
        self.height
    }

    /// Return the user-space width divided by height.
    pub fn aspect_ratio(self) -> f64 {
        self.width / self.height
    }
}

impl std::fmt::Display for ViewBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} {} {}",
            self.left, self.top, self.width, self.height
        )
    }
}

pub(in crate::svg) fn parse_view_box(val: &str) -> Result<ViewBox, InvalidSvg> {
    let mut nums = svgtypes::NumberListParser::from(val);
    let mut coords = [0.0; 4];
    for c in &mut coords {
        *c = nums
            .next()
            .ok_or(InvalidSvg::Geometry)?
            .map_err(|_| InvalidSvg::Geometry)?;
    }
    if nums.next().is_some() {
        return Err(InvalidSvg::Geometry);
    }
    let [left, top, w, h] = coords;
    ViewBox::new(left, top, w, h)
}

pub(in crate::svg) fn absolute_dimension(val: &str) -> Result<f64, InvalidSvg> {
    use svgtypes::LengthUnit;
    let len = svgtypes::Length::from_str(val.trim()).map_err(|_| InvalidSvg::Geometry)?;
    let scale = match len.unit {
        LengthUnit::None | LengthUnit::Px => 1.0,
        LengthUnit::In => CSS_PIXELS_PER_INCH,
        LengthUnit::Cm => CSS_PIXELS_PER_INCH / 2.54,
        LengthUnit::Mm => CSS_PIXELS_PER_INCH / 25.4,
        LengthUnit::Pt => CSS_PIXELS_PER_INCH / 72.0,
        LengthUnit::Pc => CSS_PIXELS_PER_INCH / 6.0,
        LengthUnit::Em | LengthUnit::Ex | LengthUnit::Percent => return Err(InvalidSvg::Geometry),
    };
    let num = len.number * scale;
    (num.is_finite() && num > 0.0)
        .then_some(num)
        .ok_or(InvalidSvg::Geometry)
}
