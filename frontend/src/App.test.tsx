import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";

function renderApp() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false }
    }
  });

  render(
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  );
}

function jsonResponse(body: unknown) {
  return Promise.resolve(
    new Response(JSON.stringify(body), {
      status: 200,
      headers: { "Content-Type": "application/json" }
    })
  );
}

describe("App", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("renders login when unauthenticated", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(await jsonResponse({ authenticated: false }));

    renderApp();

    expect(await screen.findByRole("button", { name: /log in/i })).toBeInTheDocument();
  });

  it("renders dashboard data when authenticated", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation((input) => {
      const path = input.toString();
      if (path.endsWith("/api/auth/me")) {
        return jsonResponse({ authenticated: true, username: "admin" });
      }
      if (path.endsWith("/api/activities")) {
        return jsonResponse([
          {
            id: 1,
            strava_activity_id: 100,
            name: "Morning run",
            sport_type: "Run",
            start_date: "2026-04-20T12:00:00Z",
            moving_time_seconds: 1800,
            distance_meters: 8046.72,
            average_heartrate: 145,
            total_elevation_gain: 50,
            deleted_at: null,
            private_unavailable: 0
          }
        ]);
      }
      if (path.endsWith("/api/advice")) {
        return jsonResponse([
          {
            id: 1,
            provider: "openai",
            model: "gpt-4.1-mini",
            input_window_days: 28,
            created_at: "2026-04-20T12:00:00Z",
            body: {
              summary: "Keep the easy volume steady.",
              load_observations: ["consistent"],
              risks: ["sharp increases"],
              next_7_days: ["Easy run"],
              recovery_notes: "Sleep well.",
              confidence: 0.7,
              safety_note: "Not medical advice."
            }
          }
        ]);
      }
      if (path.endsWith("/api/strava/status")) {
        return jsonResponse({
          configured: true,
          connected: true,
          athlete: {
            strava_athlete_id: 123,
            firstname: "Alex",
            lastname: "Runner"
          },
          scopes: ["read", "activity:read"],
          token_expires_at: 1770000000,
          queued_jobs: 0,
          running_jobs: 0,
          failed_jobs: 0,
          last_completed_sync_at: null
        });
      }
      return Promise.reject(new Error(`unexpected request: ${path}`));
    });

    renderApp();

    expect(await screen.findByText("Strava connected")).toBeInTheDocument();
    expect(await screen.findByText("Morning run")).toBeInTheDocument();
    expect(await screen.findByText("Keep the easy volume steady.")).toBeInTheDocument();
  });
});
