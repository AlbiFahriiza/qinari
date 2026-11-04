/** Convert an arbitrary string into a URL-safe slug. */
export function slugify(input: string): string {
  return (
    input
      .normalize("NFKD")
      // Remove combining marks produced by NFKD so accents fold to ASCII.
      .replace(/[\u0300-\u036f]/g, "")
      .toLowerCase()
      .trim()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
  );
}
