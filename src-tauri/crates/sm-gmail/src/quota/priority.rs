#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    P1 = 1,
    P2 = 2,
    P3 = 3,
    P4 = 4,
    P5 = 5,
    P6 = 6,
}

impl Priority {
    #[must_use]
    pub fn is_background(self) -> bool {
        self >= Self::P4
    }
}
