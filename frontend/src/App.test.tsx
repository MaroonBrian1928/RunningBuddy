import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { format, startOfMonth } from "date-fns";

const TODAY = format(new Date(), "yyyy-MM-dd");
// A second stored day guaranteed to be inside the visible month grid.
const OTHER_DAY = TODAY.endsWith("-01")
  ? format(new Date(startOfMonth(new Date()).getTime() + 86400000), "yyyy-MM-dd")
  : format(startOfMonth(new Date()), "yyyy-MM-dd");

function adviceBody(summary: string) {
  return {
    summary,
    load_observations: ["consistent"],
    risks: ["sharp increases"],
    last_7_days: ["Solid easy week behind you"],
    next_7_days: ["Easy run"],
    recovery_notes: "Sleep well.",
    confidence: 0.7
  };
}

function storedAdvice(id: number, date: string, summary: string) {
  return {
    id,
    activity_id: null,
    provider: "openai",
    model: "gpt-4.1-mini",
    input_window_days: 28,
    advice_date: date,
    created_at: `${date}T12:00:00Z`,
    body: adviceBody(summary)
  };
}

const queryClients: QueryClient[] = [];

function renderApp() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false }
    }
  });
  queryClients.push(queryClient);

  render(
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  );
  return queryClient;
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
    cleanup();
    queryClients.splice(0).forEach((queryClient) => queryClient.clear());
    vi.restoreAllMocks();
  });

  it("renders login when unauthenticated", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValueOnce(await jsonResponse({ authenticated: false }));

    renderApp();

    expect(await screen.findByRole("button", { name: /log in/i })).toBeInTheDocument();
  });

  it("shows password and reports incorrect credentials accessibly", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation((input) => input.toString().endsWith("/login")
      ? Promise.resolve(new Response(JSON.stringify({ error: "unauthorized" }), { status: 401 }))
      : jsonResponse({ authenticated: false }));
    renderApp();
    const password = await screen.findByLabelText("Password", { exact: true });
    expect(password).toHaveAttribute("autocomplete", "current-password");
    expect(screen.getByRole("button", { name: /^log in$/i })).toBeDisabled();
    fireEvent.change(password, { target: { value: "wrong-password" } });
    fireEvent.click(screen.getByRole("button", { name: "Show password" }));
    expect(password).toHaveAttribute("type", "text");
    fireEvent.click(screen.getByRole("button", { name: /^log in$/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Incorrect username or password");
  });

  it("uses the login response and clears private queries on logout", async () => {
    const fetch = vi.spyOn(globalThis, "fetch").mockImplementation((input) => {
      const path = input.toString();
      if (path.endsWith("/me")) return jsonResponse({ authenticated: false });
      if (path.endsWith("/login")) return jsonResponse({ authenticated: true, username: "runner" });
      if (path.endsWith("/logout")) return Promise.resolve(new Response(null, { status: 204 }));
      if (path.endsWith("/strava/status")) return jsonResponse({ configured: false, connected: false, scopes: [], queued_jobs: 0, running_jobs: 0, failed_jobs: 0 });
      return jsonResponse([]);
    });
    const client = renderApp();
    fireEvent.change(await screen.findByLabelText("Password", { exact: true }), { target: { value: "password" } });
    fireEvent.click(screen.getByRole("button", { name: /^log in$/i }));
    expect(await screen.findByRole("heading", { name: "Every run adds up." })).toBeInTheDocument();
    expect(fetch.mock.calls.filter(([path]) => path.toString().endsWith("/me"))).toHaveLength(1);
    client.setQueryData(["private-test"], { secret: "private activity" });
    fireEvent.click(screen.getByRole("button", { name: "Log out" }));
    expect(await screen.findByRole("heading", { name: "Welcome back" })).toBeInTheDocument();
    expect(client.getQueryData(["private-test"])).toBeUndefined();
    expect(screen.queryByText(/Your session has ended/)).not.toBeInTheDocument();
  });

  it("returns to login and clears cached data when a protected request returns 401", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation((input) => {
      const path = input.toString();
      if (path.endsWith("/me")) return jsonResponse({ authenticated: true, username: "runner" });
      if (path.endsWith("/activities")) return Promise.resolve(new Response(JSON.stringify({ error: "unauthorized" }), { status: 401 }));
      if (path.endsWith("/strava/status")) return jsonResponse({ connected: false, scopes: [], queued_jobs: 0, running_jobs: 0, failed_jobs: 0 });
      return jsonResponse([]);
    });
    const client = renderApp();
    client.setQueryData(["private-test"], "private");
    expect(await screen.findByRole("heading", { name: "Welcome back" })).toBeInTheDocument();
    expect(screen.getByText(/Your session has ended/)).toBeInTheDocument();
    expect(client.getQueryData(["private-test"])).toBeUndefined();
  });

  it("offers a retry when the session check fails", async () => {
    vi.spyOn(globalThis, "fetch")
      .mockRejectedValueOnce(new TypeError("Failed to fetch"))
      .mockImplementation(() => jsonResponse({ authenticated: false }));
    renderApp();
    expect(await screen.findByRole("alert")).toHaveTextContent("Can’t reach RunningBuddy");
    expect(screen.queryByLabelText("Password", { exact: true })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("heading", { name: "Welcome back" })).toBeInTheDocument();
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
      if (path.endsWith("/api/advice/calendar")) {
        return jsonResponse([{ id: 1, advice_date: TODAY, created_at: `${TODAY}T12:00:00Z` }]);
      }
      if (path.endsWith(`/api/advice/day/${TODAY}`)) {
        return jsonResponse({
          advice: storedAdvice(1, TODAY, "Keep the easy volume steady."),
          chat: []
        });
      }
      if (path.endsWith("/api/advice/1/chat")) {
        return jsonResponse({
          message: "Keep it easy tomorrow.",
          chat: [
            { id: 1, role: "user", content: "How hard should tomorrow be?", created_at: `${TODAY}T12:05:00Z` },
            { id: 2, role: "assistant", content: "Keep it easy tomorrow.", created_at: `${TODAY}T12:05:01Z` }
          ]
        });
      }
      if (path.endsWith("/api/advice")) {
        return jsonResponse([storedAdvice(1, TODAY, "Keep the easy volume steady.")]);
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
          last_completed_sync_at: "2026-04-20 12:01:00"
        });
      }
      return Promise.reject(new Error(`unexpected request: ${path}`));
    });

    renderApp();

    // Collapsed view: last sync time (rendered in the browser's zone) and the
    // Sync button, nothing else about the connection.
    const lastSync = format(new Date(Date.UTC(2026, 3, 20, 12, 1)), "MMM d, h:mm a");
    expect(await screen.findByText(`Last sync ${lastSync}`)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^sync$/i })).toBeEnabled();
    expect(screen.queryByText("Strava connected")).not.toBeInTheDocument();
    expect(screen.queryByText("OAuth configured")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /connect strava/i })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /configure athlete/i }));
    expect(await screen.findByText("Strava connected")).toBeInTheDocument();
    expect(screen.getByText("OAuth configured")).toBeInTheDocument();
    expect(await screen.findByText("Morning run")).toBeInTheDocument();
    expect(await screen.findByText("Keep the easy volume steady.")).toBeInTheDocument();
    expect(screen.queryByText("Not medical advice.")).not.toBeInTheDocument();
    expect(screen.getByText(/general training guidance only/i)).toBeInTheDocument();

    fireEvent.change(screen.getByPlaceholderText("Ask a follow-up..."), {
      target: { value: "How hard should tomorrow be?" }
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByText("Keep it easy tomorrow.")).toBeInTheDocument();
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
      if (path.endsWith("/api/advice/calendar")) {
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

    fireEvent.click(await screen.findByRole("button", { name: /configure athlete/i }));
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

  it("keeps Strava sync status live after queuing a manual sync", async () => {
    let statusCalls = 0;
    let activitiesCalls = 0;
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
      const path = input.toString();
      if (path.endsWith("/api/auth/me")) {
        return jsonResponse({ authenticated: true, username: "admin" });
      }
      if (path.endsWith("/api/activities")) {
        activitiesCalls += 1;
        return jsonResponse(activitiesCalls > 1
          ? [
              {
                id: 1,
                strava_activity_id: 100,
                name: "Fresh sync run",
                sport_type: "Run",
                start_date: "2026-04-20T12:00:00Z",
                moving_time_seconds: 1800,
                distance_meters: 8046.72,
                average_heartrate: 145,
                total_elevation_gain: 50,
                deleted_at: null,
                private_unavailable: 0
              }
            ]
          : []);
      }
      if (path.endsWith("/api/advice/calendar")) {
        return jsonResponse([]);
      }
      if (path.endsWith("/api/advice")) {
        return jsonResponse([]);
      }
      if (path.endsWith("/api/strava/status")) {
        statusCalls += 1;
        return jsonResponse({
          configured: true,
          connected: true,
          athlete: null,
          scopes: ["read", "activity:read"],
          queued_jobs: 0,
          running_jobs: 0,
          failed_jobs: 0,
          last_completed_sync_at: statusCalls > 1 ? "2026-04-20T12:01:00Z" : null
        });
      }
      if (path.endsWith("/api/strava/sync") && init?.method === "POST") {
        return Promise.resolve(new Response(null, { status: 204 }));
      }
      return Promise.reject(new Error(`unexpected request: ${path}`));
    });

    renderApp();

    fireEvent.click(await screen.findByRole("button", { name: /^sync$/i }));

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        "/api/strava/sync",
        expect.objectContaining({ method: "POST" })
      );
    });
    expect(await screen.findByText("Fresh sync run")).toBeInTheDocument();
    // Scoped to the metric tiles: calendar day numbers also render bare digits.
    const metrics = document.querySelector(".metrics") as HTMLElement;
    expect(within(metrics).getByText("1")).toBeInTheDocument();
    expect(within(metrics).getByText("5.0")).toBeInTheDocument();
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
            last_7_days: ["Good consistency this past week"],
            next_7_days: ["Keep tomorrow easy"],
            recovery_notes: "Fuel and hydrate after the run.",
            confidence: 0.8
          }
        });
      }
      if (path.endsWith("/api/advice/calendar")) {
        return jsonResponse([]);
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
    expect(screen.getByText("Pace")).toBeInTheDocument();
    expect(screen.getByText("6:00 /mi")).toBeInTheDocument();
    expect(screen.getByText("Max heart rate")).toBeInTheDocument();
    expect(screen.getByText("170 bpm")).toBeInTheDocument();
    expect(screen.getByText("Cadence")).toBeInTheDocument();
    expect(screen.getByText("164 spm")).toBeInTheDocument();
    expect(screen.queryByText("Elevation")).not.toBeInTheDocument();
    expect(screen.queryByText("Visibility")).not.toBeInTheDocument();
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
  it("browses stored overviews by day and shows each day's saved chat", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation((input) => {
      const path = input.toString();
      if (path.endsWith("/api/auth/me")) {
        return jsonResponse({ authenticated: true, username: "admin" });
      }
      if (path.endsWith("/api/activities")) {
        return jsonResponse([]);
      }
      if (path.endsWith("/api/advice/calendar")) {
        return jsonResponse([
          { id: 2, advice_date: TODAY, created_at: `${TODAY}T12:00:00Z` },
          { id: 1, advice_date: OTHER_DAY, created_at: `${OTHER_DAY}T12:00:00Z` }
        ]);
      }
      if (path.endsWith(`/api/advice/day/${TODAY}`)) {
        return jsonResponse({ advice: storedAdvice(2, TODAY, "Today's overview."), chat: [] });
      }
      if (path.endsWith(`/api/advice/day/${OTHER_DAY}`)) {
        return jsonResponse({
          advice: storedAdvice(1, OTHER_DAY, "That day's overview."),
          chat: [
            { id: 5, role: "user", content: "Was that long run too fast?", created_at: `${OTHER_DAY}T13:00:00Z` },
            { id: 6, role: "assistant", content: "It was a touch quick.", created_at: `${OTHER_DAY}T13:00:01Z` }
          ]
        });
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
      return Promise.reject(new Error(`unexpected request: ${path}`));
    });

    renderApp();

    // Defaults to the newest stored day.
    expect(await screen.findByText("Today's overview.")).toBeInTheDocument();

    const storedDayLabel = `${format(new Date(`${OTHER_DAY}T12:00:00`), "MMMM d, yyyy")} (overview saved)`;
    fireEvent.click(await screen.findByLabelText(storedDayLabel));

    expect(await screen.findByText("That day's overview.")).toBeInTheDocument();
    expect(screen.getByText("Was that long run too fast?")).toBeInTheDocument();
    expect(screen.getByText("It was a touch quick.")).toBeInTheDocument();
    expect(screen.queryByText("Today's overview.")).not.toBeInTheDocument();
  });
  it("picks up an activity advice the server finished after the request died", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let generateAttempts = 0;
    let adviceListCalls = 0;
    vi.spyOn(globalThis, "fetch").mockImplementation((input, init) => {
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
          raw_activity_json: "{}"
        });
      }
      // The connection dies, but the server keeps generating and stores the row.
      if (path.endsWith("/api/advice") && init?.method === "POST") {
        generateAttempts += 1;
        return Promise.reject(new TypeError("Failed to fetch"));
      }
      if (path.endsWith("/api/advice/calendar")) {
        return jsonResponse([]);
      }
      if (path.endsWith("/api/advice")) {
        adviceListCalls += 1;
        // Appears only once the server has finished writing it.
        return jsonResponse(
          adviceListCalls > 2
            ? [
                {
                  ...storedAdvice(9, TODAY, "Recovered activity advice."),
                  activity_id: 1
                }
              ]
            : []
        );
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
      return Promise.reject(new Error(`unexpected request: ${path}`));
    });

    renderApp();

    fireEvent.click(await screen.findByText("Morning run"));
    fireEvent.click(await screen.findByRole("button", { name: /get activity advice/i }));

    expect(await screen.findByText(/checking whether the coach finished/i)).toBeInTheDocument();

    await vi.advanceTimersByTimeAsync(10_000);

    expect(await screen.findByText("Recovered activity advice.")).toBeInTheDocument();
    expect(screen.queryByText(/checking whether the coach finished/i)).not.toBeInTheDocument();
    expect(screen.queryByText("Failed to fetch")).not.toBeInTheDocument();
    // Recovery must not pay for a second generation.
    expect(generateAttempts).toBe(1);

    vi.useRealTimers();
  });

  it("greys out the sync button while a sync is running", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation((input) => {
      const path = input.toString();
      if (path.endsWith("/api/auth/me")) {
        return jsonResponse({ authenticated: true, username: "admin" });
      }
      if (path.endsWith("/api/activities")) {
        return jsonResponse([]);
      }
      if (path.endsWith("/api/advice/calendar")) {
        return jsonResponse([]);
      }
      if (path.endsWith("/api/advice")) {
        return jsonResponse([]);
      }
      if (path.endsWith("/api/strava/status")) {
        return jsonResponse({
          configured: true,
          connected: true,
          athlete: null,
          scopes: ["read"],
          queued_jobs: 1,
          running_jobs: 0,
          failed_jobs: 0,
          last_completed_sync_at: "2026-04-20 12:01:00"
        });
      }
      return Promise.reject(new Error(`unexpected request: ${path}`));
    });

    renderApp();

    const syncButton = await screen.findByRole("button", { name: /syncing/i });
    expect(syncButton).toBeDisabled();
    expect(screen.getByText("Syncing with Strava...")).toBeInTheDocument();
    expect(screen.queryByText(/^Last sync/)).not.toBeInTheDocument();
  });
});
