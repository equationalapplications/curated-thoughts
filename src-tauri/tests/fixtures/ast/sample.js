function topFn(x) {
  return x + 1;
}

export const arrowFn = (n) => n * 2;

class Vehicle {
  constructor(make, model) {
    this.make = make;
    this.model = model;
  }

  describe() {
    return `${this.make} ${this.model}`;
  }
}
