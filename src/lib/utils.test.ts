import { describe, expect, it } from "vitest";
import { cn } from "./utils";

describe("cn", () => {
  it("joins class names", () => {
    expect(cn("flex", "items-center")).toBe("flex items-center");
  });

  it("lets later classes override earlier conflicting ones", () => {
    expect(cn("p-2", "p-4")).toBe("p-4");
  });

  it("skips falsy values", () => {
    expect(cn("flex", false && "hidden", undefined)).toBe("flex");
  });
});
