import { describe, expect, it } from "vitest";
import { containerId, formatBytes, publishedPorts, redactSensitive, summarizeError } from "./format";

describe("format helpers", () => {
  it("formats bytes with practical units", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1_024)).toBe("1.0 KB");
    expect(formatBytes(1_048_576)).toBe("1.0 MB");
  });

  it("reads container ids from top-level or configuration", () => {
    expect(containerId({ id: "top" })).toBe("top");
    expect(containerId({ configuration: { id: "config" } })).toBe("config");
  });

  it("summarizes port mappings", () => {
    expect(
      publishedPorts([
        {
          hostAddress: "127.0.0.1",
          hostPort: 8080,
          containerPort: 80,
          proto: "tcp",
        },
      ]),
    ).toBe("127.0.0.1:8080 -> 80/tcp");
  });

  it("prefers stderr in command errors", () => {
    expect(
      summarizeError({
        kind: "command_failed",
        message: "failed",
        command: "container list",
        exitCode: 1,
        stderr: "service unavailable",
      }),
    ).toContain("service unavailable");
  });

  it("redacts sensitive lines in visible text", () => {
    const redacted = redactSensitive("hello\nTOKEN=abc\npassword=secret");
    expect(redacted).toContain("hello");
    expect(redacted).not.toContain("abc");
    expect(redacted).not.toContain("secret");
  });

  it("does not hide ordinary mentions of token words", () => {
    expect(redactSensitive("approval token has expired")).toBe("approval token has expired");
  });

  it("redacts broader credential shapes", () => {
    const redacted = redactSensitive([
      "Authorization: Bearer abc",
      "API_KEY=def",
      "https://user:pass@example.com/path",
      "-----BEGIN PRIVATE KEY-----",
      "abc123",
      "-----END PRIVATE KEY-----",
    ].join("\n"));
    expect(redacted).not.toContain("Bearer abc");
    expect(redacted).not.toContain("def");
    expect(redacted).not.toContain("user:pass");
    expect(redacted).not.toContain("abc123");
  });
});
