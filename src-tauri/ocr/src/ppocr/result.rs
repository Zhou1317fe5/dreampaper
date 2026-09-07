#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone)]
pub struct TextBox {
    pub points: Vec<Point>,
    pub score: f32,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Angle {
    pub index: i32,
    pub score: f32,
}

#[derive(Debug, Default, Clone)]
pub struct TextLine {
    pub text: String,
    pub text_score: f32,
}
