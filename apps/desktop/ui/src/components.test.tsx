// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { Button, Card, EmptyState, Loading, Notice, StatusBadge } from "./components";

afterEach(cleanup);

test("a status badge carries text and a decorative icon, never color alone", () => {
  render(<StatusBadge tone="warning">Working, with problems</StatusBadge>);
  const badge = screen.getByText("Working, with problems");
  expect(badge.className).toContain("badge-warning");
  const icon = badge.querySelector(".badge-icon");
  expect(icon?.getAttribute("aria-hidden")).toBe("true");
  expect(icon?.textContent).toBe("!");
});

test("errors are alerts, other notices are polite status, and static ones are neither", () => {
  render(
    <>
      <Notice tone="error" title="Something went wrong" />
      <Notice tone="ok">Saved</Notice>
      <Notice tone="info" live={false}>
        Background note
      </Notice>
    </>,
  );
  expect(screen.getByRole("alert").textContent).toContain("Something went wrong");
  expect(screen.getByRole("status").textContent).toContain("Saved");
  expect(screen.getByText("Background note").closest("[role]")).toBeNull();
});

test("buttons default to type=button and carry their variant", () => {
  render(
    <form>
      <Button variant="danger">Remove</Button>
      <Button type="submit" variant="primary">
        Save
      </Button>
    </form>,
  );
  const remove = screen.getByRole("button", { name: "Remove" });
  expect(remove.getAttribute("type")).toBe("button");
  expect(remove.className).toBe("btn btn-danger");
  expect(screen.getByRole("button", { name: "Save" }).getAttribute("type")).toBe("submit");
});

test("cards are labelled regions; empty and loading states are distinct", () => {
  render(
    <Card id="h" heading="Devices">
      <EmptyState>No devices yet.</EmptyState>
      <Loading />
    </Card>,
  );
  expect(screen.getByRole("region", { name: "Devices" })).toBeTruthy();
  expect(screen.getByText("No devices yet.").className).toBe("empty");
  expect(screen.getByRole("status").getAttribute("aria-busy")).toBe("true");
});
