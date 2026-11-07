// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { App } from "./App";

afterEach(cleanup);

test("renders the BlueRoute shell without any network actions", () => {
  render(<App />);
  expect(screen.getByRole("heading", { name: "BlueRoute" })).toBeTruthy();
  expect(screen.queryByRole("button")).toBeNull();
});
