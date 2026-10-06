//! The SVG view box in user-space coordinates.

use std::str::FromStr;

use super::InvalidSvg;

const CSS_PIXELS_PER_INCH: f64 = 96.0;

/// A validated SVG view box in user-space coordinates.
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
            .all(|number| number.is_finite())
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

    /// The minimum x of the view box.
    pub fn left(self) -> f64 {
        self.left
    }

    /// The minimum y of the view box.
    pub fn top(self) -> f64 {
        self.top
    }

    /// The user-space width, always finite and positive.
    pub fn width(self) -> f64 {
        self.width
    }

    /// The user-space height, always finite and positive.
    pub fn height(self) -> f64 {
        self.height
    }

    /// The user-space width divided by height, which can differ from the viewport's aspect ratio.
    pub fn aspect_ratio(self) -> f64 {
        self.width / self.height
    }
}

impl std::fmt::Display for ViewBox {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} {} {} {}",
            self.left, self.top, self.width, self.height
        )
    }
}

pub(in crate::svg) fn parse_view_box(value: &str) -> Result<ViewBox, InvalidSvg> {
    let mut numbers = svgtypes::NumberListParser::from(value);
    let mut coordinates = [0.0; 4];
    for coordinate in &mut coordinates {
        *coordinate = numbers
            .next()
            .ok_or(InvalidSvg::Geometry)?
            .map_err(|_| InvalidSvg::Geometry)?;
    }
    if numbers.next().is_some() {
        return Err(InvalidSvg::Geometry);
    }
    let [left, top, width, height] = coordinates;
    ViewBox::new(left, top, width, height)
}

pub(in crate::svg) fn absolute_dimension(value: &str) -> Result<f64, InvalidSvg> {
    use svgtypes::LengthUnit;
    let length = svgtypes::Length::from_str(value.trim()).map_err(|_| InvalidSvg::Geometry)?;
    let multiplier = match length.unit {
        LengthUnit::None | LengthUnit::Px => 1.0,
        LengthUnit::In => CSS_PIXELS_PER_INCH,
        LengthUnit::Cm => CSS_PIXELS_PER_INCH / 2.54,
        LengthUnit::Mm => CSS_PIXELS_PER_INCH / 25.4,
        LengthUnit::Pt => CSS_PIXELS_PER_INCH / 72.0,
        LengthUnit::Pc => CSS_PIXELS_PER_INCH / 6.0,
        LengthUnit::Em | LengthUnit::Ex | LengthUnit::Percent => return Err(InvalidSvg::Geometry),
    };
    let number = length.number * multiplier;
    (number.is_finite() && number > 0.0)
        .then_some(number)
        .ok_or(InvalidSvg::Geometry)
}
