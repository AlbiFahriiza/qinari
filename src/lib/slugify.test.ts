import { describe, expect, it } from "vitest";
import { slugify } from "./slugify";

describe("slugify", () => {
  it("lowercases and hyphenates spaces", () => {
    expect(slugify("Qinari Starter Pack")).toBe("qinari-starter-pack");
  });

  it("folds diacritics to ASCII", () => {
    expect(slugify("Résumé Builder")).toBe("resume-builder");
  });

  it("collapses punctuation runs into a single hyphen", () => {
    expect(slugify("Hello,   World!")).toBe("hello-world");
  });

  it("trims leading and trailing hyphens", () => {
    expect(slugify("--already-slugged--")).toBe("already-slugged");
  });
});
