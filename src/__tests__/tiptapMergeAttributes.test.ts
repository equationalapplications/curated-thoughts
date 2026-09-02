import { describe, it, expect } from "vitest";
import { mergeAttributes } from "@tiptap/core";

/**
 * Regression test for GHSA-cp6q-959q-f8rh (Dependabot alert #50).
 *
 * `mergeAttributes()` assigned keys from `Object.entries()` with plain bracket
 * assignment, so an own `__proto__` key in attacker-controlled JSON replaced the
 * merged object's prototype instead of becoming a normal property. Own-property
 * checks (`Object.keys`) then showed nothing, but ProseMirror's
 * `DOMSerializer.renderSpec()` enumerates attribute objects with `for...in`,
 * which walks the prototype chain — so the injected values reached
 * `setAttribute()` on the rendered element.
 *
 * This test asserts the security property directly rather than asserting a
 * version number, so it keeps failing if the pnpm override in package.json is
 * ever removed or rolled back to a vulnerable release.
 *
 * Fixed in @tiptap/core 3.30.4; this repo pins the whole suite at 3.30.6.
 */
describe("mergeAttributes prototype manipulation (GHSA-cp6q-959q-f8rh)", () => {
  // JSON.parse is required: an object literal would treat __proto__ as a
  // setter, not as the own property an attacker actually delivers over the wire.
  const hostileAttributes = () =>
    JSON.parse('{"__proto__":{"src":"https://evil.test/x.png","onerror":"alert(1)"}}');

  it("does not expose injected values through for...in enumeration", () => {
    const merged = mergeAttributes(hostileAttributes());

    // This is the exact enumeration DOMSerializer.renderSpec() performs.
    const enumerated: string[] = [];
    for (const key in merged) {
      enumerated.push(key);
    }

    expect(enumerated).not.toContain("src");
    expect(enumerated).not.toContain("onerror");
  });

  it("does not resolve injected values through the prototype chain", () => {
    const merged = mergeAttributes(hostileAttributes()) as Record<string, unknown>;

    expect(merged.src).toBeUndefined();
    expect(merged.onerror).toBeUndefined();
  });

  it("leaves Object.prototype untouched", () => {
    mergeAttributes(hostileAttributes());

    expect(({} as Record<string, unknown>).src).toBeUndefined();
  });

  it("still merges ordinary attributes", () => {
    const merged = mergeAttributes({ class: "a" }, { "data-x": "1" });

    expect(merged).toMatchObject({ class: "a", "data-x": "1" });
  });
});
