import "@testing-library/jest-dom/vitest";
import React from "react";
import { vi } from "vitest";

vi.mock("react-map-gl/maplibre", () => ({
  default: ({ children, "aria-label": ariaLabel }: { children?: React.ReactNode; "aria-label"?: string }) =>
    React.createElement("div", { "aria-label": ariaLabel, "data-testid": "activity-map" }, children),
  Layer: () => null,
  Source: ({ children }: { children?: React.ReactNode }) =>
    React.createElement(React.Fragment, null, children)
}));

class ResizeObserverMock {
  observe() {}
  unobserve() {}
  disconnect() {}
}

globalThis.ResizeObserver = ResizeObserverMock;

Element.prototype.getBoundingClientRect = function () {
  return {
    width: 1024,
    height: 768,
    top: 0,
    left: 0,
    bottom: 768,
    right: 1024,
    x: 0,
    y: 0,
    toJSON: () => {}
  };
};
