export function topFn(x: number): number {
  return x + 1;
}

export const arrowFn = (n: number) => n * 2;

export interface Shape {
  area(): number;
}

export type Color = "red" | "green" | "blue";

export class Rectangle {
  constructor(private width: number, private height: number) {}

  area(): number {
    return this.width * this.height;
  }

  perimeter(): number {
    return 2 * (this.width + this.height);
  }
}
