import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
    cleanup();
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

  it("normalizes plan start timestamps before saving", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
      const path = input.toString();
      if (path.endsWith("/api/auth/me")) {
        return jsonResponse({
          authenticated: true,
          username: "admin",
          training_plan: "Base block",
          training_goals: "10K",
          plan_start_date: "2026-04-25T12:00:00Z"
        });
      }
      if (path.endsWith("/api/activities")) {
        return jsonResponse([]);
      }
      if (path.endsWith("/api/advice")) {
        return jsonResponse([]);
      }
      if (path.endsWith("/api/strava/status")) {
        return jsonResponse({
          configured: true,
          connected: false,
          athlete: null,
          scopes: [],
          queued_jobs: 0,
          running_jobs: 0,
          failed_jobs: 0,
          last_completed_sync_at: null
        });
      }
      if (path.endsWith("/api/auth/training-plan") && init?.method === "PUT") {
        return jsonResponse({});
      }
      return Promise.reject(new Error(`unexpected request: ${path}`));
    });

    renderApp();

    const startDate = await screen.findByLabelText(/start date/i);
    await waitFor(() => expect(startDate).toHaveValue("2026-04-25"));

    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/api/auth/training-plan",
        expect.objectContaining({
          body: expect.stringContaining("\"plan_start_date\":\"2026-04-25\"")
        })
      );
    });
  });

  it("generates and shows activity-specific advice in the activity detail panel", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
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
      if (path.endsWith("/api/activities/1")) {
        return jsonResponse({
          id: 1,
          strava_activity_id: 100,
          name: "Morning run",
          sport_type: "Run",
          start_date: "2026-04-20T12:00:00Z",
          elapsed_time_seconds: 1900,
          moving_time_seconds: 1800,
          distance_meters: 8046.72,
          total_elevation_gain: 50,
          average_heartrate: 145,
          max_heartrate: 170,
          average_speed: 3.8,
          max_speed: 5.2,
          average_cadence: 82,
          average_watts: null,
          kilojoules: null,
          suffer_score: null,
          visibility: "everyone",
          deleted_at: null,
          private_unavailable: 0,
          raw_activity_json: JSON.stringify({
            map: {
              summary_polyline: "_p~iF~ps|U_ulLnnqC_mqNvxq`@"
            }
          })
        });
      }
      if (path.endsWith("/api/advice") && init?.method === "POST") {
        return jsonResponse({
          id: 2,
          activity_id: 1,
          provider: "openai",
          model: "gpt-4.1-mini",
          input_window_days: 28,
          created_at: "2026-04-20T12:00:00Z",
          body: {
            summary: "This run fits well as aerobic maintenance.",
            load_observations: ["steady effort"],
            risks: ["watch the next hard day"],
            next_7_days: ["Keep tomorrow easy"],
            recovery_notes: "Fuel and hydrate after the run.",
            confidence: 0.8,
            safety_note: "Not medical advice."
          }
        });
      }
      if (path.endsWith("/api/advice")) {
        return jsonResponse([]);
      }
      if (path.endsWith("/api/strava/status")) {
        return jsonResponse({
          configured: true,
          connected: true,
          athlete: null,
          scopes: [],
          queued_jobs: 0,
          running_jobs: 0,
          failed_jobs: 0,
          last_completed_sync_at: null
        });
      }
      return Promise.reject(new Error(`unexpected request: ${path}`));
    });

    renderApp();

    fireEvent.click(await screen.findByText("Morning run"));
    expect(await screen.findByLabelText("Morning run route map")).toBeInTheDocument();
    expect(await screen.findByText("No advice generated for this activity yet.")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /get activity advice/i }));

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/api/advice",
        expect.objectContaining({
          body: expect.stringContaining("\"activity_id\":1"),
          method: "POST"
        })
      );
    });
    expect(await screen.findByText("This run fits well as aerobic maintenance.")).toBeInTheDocument();
  });
});
