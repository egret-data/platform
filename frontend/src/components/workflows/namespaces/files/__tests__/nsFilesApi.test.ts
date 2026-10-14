import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/client";
import { fileErrorMessage } from "../nsFilesApi";

describe("fileErrorMessage", () => {
  it("drops the server's `conflict: ` prefix from a 409's sentence", () => {
    const e = new ApiError(409, "conflict: a directory exists at '/scripts/'");
    expect(fileErrorMessage(e, "The file was not saved")).toBe(
      "The file was not saved: a directory exists at '/scripts/'. Choose another path, or change what is there first.",
    );
  });
});
