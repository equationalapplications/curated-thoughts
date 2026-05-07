pub fn top_fn(x: u32) -> u32 {
    x + 1
}

pub struct Foo {
    pub x: u32,
}

pub enum Bar {
    A,
    B,
}

pub struct Counter {
    count: u32,
}

impl Counter {
    const MAX: u32 = 999;

    pub fn new() -> Self {
        Counter { count: 0 }
    }

    pub fn increment(&mut self) {
        self.count += 1;
    }

    pub fn value(&self) -> u32 {
        self.count
    }
}
