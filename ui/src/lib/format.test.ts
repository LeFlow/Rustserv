import { describe, expect, it } from "vitest";
import { formatOctets, formatPorts, groupesPhp } from "./format";

describe("formatPorts", () => {
  it("sépare les ports isolés", () => {
    expect(formatPorts([80, 443])).toBe("80, 443");
    expect(formatPorts([3306])).toBe("3306");
  });
  it("regroupe les plages consécutives, version par version", () => {
    expect(formatPorts([9850, 9851, 9852, 9853, 9740, 9741, 9742, 9743])).toBe("9850-9853, 9740-9743");
  });
  it("liste vide", () => {
    expect(formatPorts([])).toBe("");
  });
});

describe("groupesPhp", () => {
  it("regroupe les processus par version", () => {
    const g = groupesPhp([{ id: "php-8.5.13#0" }, { id: "php-8.5.13#1" }, { id: "php-7.4.33#0" }]);
    expect(g.map(([v, m]) => [v, m.length])).toEqual([["8.5.13", 2], ["7.4.33", 1]]);
  });
});

describe("formatOctets", () => {
  it("unités françaises", () => {
    expect(formatOctets(512)).toBe("512 o");
    expect(formatOctets(34185054)).toMatch(/^32,6\s?Mo$/);
  });
});
