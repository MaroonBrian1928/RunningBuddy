export type MeResponse = {
  authenticated: boolean;
  username?: string | null;
  training_plan?: string | null;
  training_goals?: string | null;
  plan_start_date?: string | null;
};

export type ActivitySummary = {
  id: number;
  strava_activity_id: number;
  name: string;
  sport_type?: string | null;
  start_date?: string | null;
  moving_time_seconds?: number | null;
  distance_meters?: number | null;
  average_heartrate?: number | null;
  total_elevation_gain?: number | null;
  deleted_at?: string | null;
  private_unavailable: number;
};

export type ActivityDetail = ActivitySummary & {
  elapsed_time_seconds?: number | null;
  max_heartrate?: number | null;
  average_speed?: number | null;
  max_speed?: number | null;
  average_cadence?: number | null;
  average_watts?: number | null;
  kilojoules?: number | null;
  suffer_score?: number | null;
  visibility?: string | null;
  raw_activity_json: string;
};

export type TrainingAdvice = {
  id: number;
  activity_id?: number | null;
  provider: string;
  model: string;
  input_window_days: number;
  created_at: string;
  body: {
    summary: string;
    load_observations: string[];
    risks: string[];
    next_7_days: string[];
    recovery_notes: string;
    confidence: number;
  };
};

export type AdviceChatMessage = {
  role: "user" | "assistant";
  content: string;
};

export type StravaStatus = {
  configured: boolean;
  connected: boolean;
  athlete?: {
    strava_athlete_id: number;
    username?: string | null;
    firstname?: string | null;
    lastname?: string | null;
    profile_url?: string | null;
  } | null;
  scopes: string[];
  token_expires_at?: number | null;
  queued_jobs: number;
  running_jobs: number;
  failed_jobs: number;
  last_completed_sync_at?: string | null;
};

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    credentials: "include",
    headers: {
      "Content-Type": "application/json",
      ...init?.headers
    },
    ...init
  });

  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new Error(body.error ?? `Request failed with ${response.status}`);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  const text = await response.text();
  if (!text) {
    return undefined as T;
  }

  return JSON.parse(text);
}

export const api = {
  me: () => request<MeResponse>("/api/auth/me"),
  login: (username: string, password: string) =>
    request<MeResponse>("/api/auth/login", {
      method: "POST",
      body: JSON.stringify({ username, password })
  }),
  logout: () => request<void>("/api/auth/logout", { method: "POST" }),
  updateTrainingPlan: (payload: {
    training_plan?: string | null;
    training_goals?: string | null;
    plan_start_date?: string | null;
  }) =>
    request<void>("/api/auth/training-plan", {
      method: "PUT",
      body: JSON.stringify(payload)
    }),
  stravaConnect: () => request<{ authorization_url: string }>("/api/strava/connect"),
  stravaStatus: () => request<StravaStatus>("/api/strava/status"),
  sync: () => request<void>("/api/strava/sync", { method: "POST" }),
  activities: () => request<ActivitySummary[]>("/api/activities"),
  activity: (id: number) => request<ActivityDetail>(`/api/activities/${id}`),
  advice: () => request<TrainingAdvice[]>("/api/advice"),
  chatAdvice: (id: number, messages: AdviceChatMessage[]) =>
    request<{ message: string }>(`/api/advice/${id}/chat`, {
      method: "POST",
      body: JSON.stringify({ messages })
    }),
  generateAdvice: (input_window_days = 28, activity_id?: number | null) =>
    request<TrainingAdvice>("/api/advice", {
      method: "POST",
      body: JSON.stringify({ input_window_days, activity_id })
    })
};
