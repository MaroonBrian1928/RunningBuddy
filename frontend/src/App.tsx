import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import Map, { Layer, Source } from "react-map-gl/maplibre";
import type { StyleSpecification } from "maplibre-gl";
import {
  Activity,
  CalendarDays,
  CheckCircle2,
  ClipboardList,
  LogOut,
  RefreshCw,
  Save,
  ShieldCheck,
  Sparkles,
  Target,
  Unplug,
  XCircle
} from "lucide-react";
import {
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis
} from "recharts";
import { format, parseISO } from "date-fns";
import {
  api,
  ActivityDetail,
  ActivitySummary,
  MeResponse,
  StravaStatus,
  TrainingAdvice,
  type AdviceChatMessage
} from "./lib/api";

const CONFIGURED_MAP_STYLE_URL =
  envValue(import.meta.env.VITE_MAP_STYLE_URL)
  ?? mapTilerStyleUrl(import.meta.env.VITE_MAPTILER_KEY)
  ?? stadiaStyleUrl(import.meta.env.VITE_STADIA_API_KEY);

const FALLBACK_MAP_STYLE: StyleSpecification = {
  version: 8,
  sources: {
    osm: {
      type: "raster",
      tiles: ["https://tile.openstreetmap.org/{z}/{x}/{y}.png"],
      tileSize: 256,
      attribution: "© OpenStreetMap contributors"
    }
  },
  layers: [
    {
      id: "osm",
      type: "raster",
      source: "osm"
    }
  ]
};

type RouteGeoJson = {
  type: "Feature";
  properties: Record<string, never>;
  geometry: {
    type: "LineString";
    coordinates: [number, number][];
  };
};

export function App() {
  const queryClient = useQueryClient();
  const [selectedActivityId, setSelectedActivityId] = useState<number | null>(null);
  const me = useQuery({ queryKey: ["me"], queryFn: api.me });
  const activities = useQuery({
    queryKey: ["activities"],
    queryFn: api.activities,
    enabled: me.data?.authenticated === true
  });
  const advice = useQuery({
    queryKey: ["advice"],
    queryFn: api.advice,
    enabled: me.data?.authenticated === true
  });
  const stravaStatus = useQuery({
    queryKey: ["stravaStatus"],
    queryFn: api.stravaStatus,
    enabled: me.data?.authenticated === true
  });
  const selectedActivity = useQuery({
    queryKey: ["activity", selectedActivityId],
    queryFn: () => api.activity(selectedActivityId!),
    enabled: selectedActivityId !== null
  });
  const generateAdvice = useMutation({
    mutationFn: (activityId?: number | null) => api.generateAdvice(28, activityId),
    onSuccess: (newAdvice) => {
      queryClient.setQueryData<TrainingAdvice[]>(["advice"], (current = []) => [
        newAdvice,
        ...current.filter((item) => item.id !== newAdvice.id)
      ]);
    }
  });
  const logout = useMutation({
    mutationFn: api.logout,
    onSuccess: () => queryClient.invalidateQueries()
  });

  if (me.isLoading) {
    return <Shell><p className="muted">Loading session...</p></Shell>;
  }

  if (!me.data?.authenticated) {
    return <Login />;
  }

  return (
    <Shell
      action={
        <button className="iconButton" onClick={() => logout.mutate()} aria-label="Log out" title="Log out">
          <LogOut size={18} />
        </button>
      }
    >
      <header className="topbar">
        <div>
          <h1>RunningBuddy</h1>
          <p>Signed in as {me.data.username}</p>
        </div>
        <StravaActions />
      </header>

      <StravaStatusPanel status={stravaStatus.data} isLoading={stravaStatus.isLoading} />
      <TrainingPlanPanel me={me.data} />
      <DashboardSummary activities={activities.data ?? []} />
      <TrendChart activities={activities.data ?? []} />

      <main className="layout">
        <ActivityTable
          activities={activities.data ?? []}
          isLoading={activities.isLoading}
          selectedId={selectedActivityId}
          onSelect={setSelectedActivityId}
        />
        <ActivityDetailPanel
          activity={selectedActivity.data}
          advice={selectedActivityId ? advice.data?.find((item) => item.activity_id === selectedActivityId) : undefined}
          generateError={generateAdvice.error}
          isGenerating={generateAdvice.isPending && generateAdvice.variables === selectedActivityId}
          isLoading={selectedActivity.isLoading}
          selectedActivityId={selectedActivityId}
          onGenerateAdvice={() => generateAdvice.mutate(selectedActivityId)}
        />
      </main>

      <AdvicePanel
        advice={advice.data ?? []}
        generateError={generateAdvice.error}
        isGenerating={generateAdvice.isPending && generateAdvice.variables == null}
        onGenerateAdvice={() => generateAdvice.mutate(null)}
      />
    </Shell>
  );
}

function Login() {
  const queryClient = useQueryClient();
  const [username, setUsername] = useState("admin");
  const [password, setPassword] = useState("");
  const login = useMutation({
    mutationFn: () => api.login(username, password),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["me"] })
  });

  return (
    <Shell>
      <form className="login" onSubmit={(event) => { event.preventDefault(); login.mutate(); }}>
        <ShieldCheck size={32} />
        <h1>RunningBuddy</h1>
        <label>
          Username
          <input value={username} onChange={(event) => setUsername(event.target.value)} />
        </label>
        <label>
          Password
          <input type="password" value={password} onChange={(event) => setPassword(event.target.value)} />
        </label>
        {login.error && <p className="error">{login.error.message}</p>}
        <button type="submit" disabled={login.isPending}>Log in</button>
      </form>
    </Shell>
  );
}

function Shell({ children, action }: { children: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div className="appShell">
      <nav>
        <div className="brand"><Activity size={20} /> RunningBuddy</div>
        {action}
      </nav>
      {children}
      <footer className="appFooter">
        RunningBuddy provides general training guidance only. It is not medical advice, diagnosis, or injury treatment.
      </footer>
    </div>
  );
}

function StravaActions() {
  const queryClient = useQueryClient();
  const connect = useMutation({
    mutationFn: api.stravaConnect,
    onSuccess: (data) => {
      window.location.href = data.authorization_url;
    }
  });
  const sync = useMutation({
    mutationFn: api.sync,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["stravaStatus"] })
  });

  return (
    <div className="toolbar">
      <button onClick={() => connect.mutate()} disabled={connect.isPending}>
        <Unplug size={16} /> Connect Strava
      </button>
      <button onClick={() => sync.mutate()} disabled={sync.isPending}>
        <RefreshCw size={16} /> Sync
      </button>
    </div>
  );
}

function StravaStatusPanel({ status, isLoading }: { status?: StravaStatus; isLoading: boolean }) {
  const athleteName = [status?.athlete?.firstname, status?.athlete?.lastname].filter(Boolean).join(" ");
  const isConnected = status?.connected === true;

  return (
    <section className="statusBand">
      <div>
        <span className={isConnected ? "statusPill connected" : "statusPill"}>
          {isConnected ? <CheckCircle2 size={16} /> : <XCircle size={16} />}
          {isLoading ? "Checking Strava" : isConnected ? "Strava connected" : "Strava not connected"}
        </span>
        <strong>{athleteName || status?.athlete?.username || "Manual sync ready"}</strong>
      </div>
      <div className="statusMeta">
        <span>{status?.configured ? "OAuth configured" : "OAuth missing"}</span>
        <span>{status?.scopes?.length ? status.scopes.join(", ") : "No scopes yet"}</span>
        <span>{status ? `${status.queued_jobs} queued / ${status.running_jobs} running / ${status.failed_jobs} failed` : "Queue unavailable"}</span>
      </div>
    </section>
  );
}

function TrainingPlanPanel({ me }: { me: MeResponse }) {
  const queryClient = useQueryClient();
  const [trainingPlan, setTrainingPlan] = useState(me.training_plan ?? "");
  const [trainingGoals, setTrainingGoals] = useState(me.training_goals ?? "");
  const [planStartDate, setPlanStartDate] = useState(toDateInputValue(me.plan_start_date));
  const save = useMutation({
    mutationFn: () => api.updateTrainingPlan({
      training_plan: trainingPlan,
      training_goals: trainingGoals,
      plan_start_date: planStartDate || null
    }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["me"] })
  });

  useEffect(() => {
    setTrainingPlan(me.training_plan ?? "");
    setTrainingGoals(me.training_goals ?? "");
    setPlanStartDate(toDateInputValue(me.plan_start_date));
  }, [me.training_plan, me.training_goals, me.plan_start_date]);

  return (
    <section className="panel trainingPlan">
      <div className="panelHeader">
        <h2>Training Plan</h2>
        <button onClick={() => save.mutate()} disabled={save.isPending}>
          <Save size={16} /> {save.isPending ? "Saving..." : "Save"}
        </button>
      </div>
      <div className="planGrid">
        <label>
          <span><Target size={16} /> Goal</span>
          <input
            value={trainingGoals}
            onChange={(event) => setTrainingGoals(event.target.value)}
            placeholder="Half marathon, base phase, comeback block..."
          />
        </label>
        <label>
          <span><CalendarDays size={16} /> Start date</span>
          <input
            type="date"
            value={planStartDate}
            onChange={(event) => setPlanStartDate(event.target.value)}
          />
        </label>
      </div>
      <label className="planText">
        <span><ClipboardList size={16} /> Plan notes</span>
        <textarea
          value={trainingPlan}
          onChange={(event) => setTrainingPlan(event.target.value)}
          placeholder="Weekly mileage, workouts, long-run progression, constraints, upcoming race..."
        />
      </label>
      {save.error && <p className="error">{(save.error as Error).message}</p>}
    </section>
  );
}

function DashboardSummary({ activities }: { activities: ActivitySummary[] }) {
  const totals = useMemo(() => {
    const distance = activities.reduce((sum, activity) => sum + (activity.distance_meters ?? 0), 0);
    const time = activities.reduce((sum, activity) => sum + (activity.moving_time_seconds ?? 0), 0);
    const runs = activities.filter((activity) => activity.deleted_at === null).length;
    return { distance, time, runs };
  }, [activities]);

  return (
    <section className="metrics">
      <Metric icon={<Activity size={18} />} label="Activities" value={totals.runs.toString()} />
      <Metric icon={<CalendarDays size={18} />} label="Miles" value={metersToMiles(totals.distance)} />
      <Metric icon={<RefreshCw size={18} />} label="Moving Time" value={secondsToTime(totals.time)} />
    </section>
  );
}

function Metric({ icon, label, value }: { icon: React.ReactNode; label: string; value: string }) {
  return (
    <div className="metric">
      <span>{icon}{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function TrendChart({ activities }: { activities: ActivitySummary[] }) {
  const data = activities
    .filter((activity) => activity.start_date && activity.distance_meters)
    .slice()
    .reverse()
    .map((activity) => ({
      date: format(parseISO(activity.start_date!), "MMM d"),
      miles: Number(metersToMiles(activity.distance_meters))
    }));

  return (
    <section className="panel">
      <h2>Distance Trend</h2>
      <div className="chart">
        <ResponsiveContainer>
          <LineChart data={data}>
            <XAxis dataKey="date" />
            <YAxis width={42} />
            <Tooltip />
            <Line type="monotone" dataKey="miles" stroke="#006d77" strokeWidth={2} dot={false} />
          </LineChart>
        </ResponsiveContainer>
      </div>
    </section>
  );
}

function ActivityTable({
  activities,
  isLoading,
  selectedId,
  onSelect
}: {
  activities: ActivitySummary[];
  isLoading: boolean;
  selectedId: number | null;
  onSelect: (id: number) => void;
}) {
  return (
    <section className="panel">
      <h2>Activities</h2>
      {isLoading && <p className="muted">Loading activities...</p>}
      {!isLoading && activities.length === 0 && <p className="muted">No synced activities yet.</p>}
      <div className="table">
        {activities.map((activity) => (
          <button
            key={activity.id}
            className={activity.id === selectedId ? "row selected" : "row"}
            onClick={() => onSelect(activity.id)}
          >
            <span>{activity.name}</span>
            <span>{activity.start_date ? format(parseISO(activity.start_date), "MMM d") : "n/a"}</span>
            <span>{metersToMiles(activity.distance_meters)} mi</span>
          </button>
        ))}
      </div>
    </section>
  );
}

function ActivityDetailPanel({
  activity,
  advice,
  generateError,
  isGenerating,
  isLoading,
  selectedActivityId,
  onGenerateAdvice
}: {
  activity?: ActivityDetail;
  advice?: TrainingAdvice;
  generateError: Error | null;
  isGenerating: boolean;
  isLoading: boolean;
  selectedActivityId: number | null;
  onGenerateAdvice: () => void;
}) {
  return (
    <section className="panel detailPanel">
      <div className="panelHeader">
        <h2>Activity Detail</h2>
        <button onClick={onGenerateAdvice} disabled={!selectedActivityId || isGenerating}>
          <Sparkles size={16} /> {isGenerating ? "Generating..." : "Get Activity Advice"}
        </button>
      </div>
      {isLoading && <p className="muted">Loading activity...</p>}
      {!selectedActivityId && <p className="muted">Select an activity to inspect streams and raw Strava detail.</p>}
      {activity && (
        <>
          <dl className="details">
            <div><dt>Name</dt><dd>{activity.name}</dd></div>
            <div><dt>Sport</dt><dd>{activity.sport_type ?? "n/a"}</dd></div>
            <div><dt>Date</dt><dd>{activity.start_date ? format(parseISO(activity.start_date), "MMM d, h:mm a") : "n/a"}</dd></div>
            <div><dt>Distance</dt><dd>{metersToMiles(activity.distance_meters)} mi</dd></div>
            <div><dt>Moving time</dt><dd>{secondsToTime(activity.moving_time_seconds)}</dd></div>
            <div><dt>Elapsed time</dt><dd>{secondsToTime(activity.elapsed_time_seconds)}</dd></div>
            <div><dt>Pace</dt><dd>{pacePerMile(activity.distance_meters, activity.moving_time_seconds)}</dd></div>
            <div><dt>Avg heart rate</dt><dd>{formatNumber(activity.average_heartrate)} bpm</dd></div>
            <div><dt>Max heart rate</dt><dd>{formatNumber(activity.max_heartrate)} bpm</dd></div>
            <div><dt>Cadence</dt><dd>{formatCadence(activity)}</dd></div>
            <div><dt>Relative effort</dt><dd>{formatNumber(activity.suffer_score)}</dd></div>
          </dl>
          <ActivityMap activity={activity} />
          <div className="activityAdvice">
            <h3>Activity Advice</h3>
            {generateError && isGenerating && <p className="error">{generateError.message}</p>}
            {!advice && <p className="muted">No advice generated for this activity yet.</p>}
            {advice && <AdviceCard advice={advice} />}
          </div>
        </>
      )}
    </section>
  );
}

function ActivityMap({ activity }: { activity: ActivityDetail }) {
  const route = useMemo(() => activityRouteGeoJson(activity), [activity]);
  const [mapStyle, setMapStyle] = useState<string | StyleSpecification>(
    CONFIGURED_MAP_STYLE_URL ?? FALLBACK_MAP_STYLE
  );
  const [styleWarning, setStyleWarning] = useState<string | null>(null);
  const [mapReady, setMapReady] = useState(false);

  if (!route) {
    return <p className="muted mapEmpty">No route map data is available for this activity.</p>;
  }

  const bounds = routeBounds(route.geometry.coordinates);

  return (
    <div className="activityMap" aria-label={`${activity.name} route map`}>
      <Map
        initialViewState={{
          bounds: [
            [bounds.minLng, bounds.minLat],
            [bounds.maxLng, bounds.maxLat]
          ],
          fitBoundsOptions: {
            maxZoom: 12,
            padding: 48
          }
        }}
        key={typeof mapStyle === "string" ? mapStyle : "fallback-osm"}
        mapStyle={mapStyle}
        attributionControl={false}
        style={{ width: "100%", height: "100%" }}
        onLoad={() => setMapReady(true)}
        onError={() => {
          if (mapStyle !== FALLBACK_MAP_STYLE) {
            setMapReady(false);
            setMapStyle(FALLBACK_MAP_STYLE);
            setStyleWarning("Map style failed to load, using the fallback basemap.");
          } else {
            setStyleWarning("Basemap tiles failed to load. Check network access or configure VITE_MAP_STYLE_URL.");
          }
        }}
      >
        <Source id="activity-route" type="geojson" data={route}>
          <Layer
            id="activity-route-glow"
            type="line"
            paint={{
              "line-color": "#fc4c02",
              "line-opacity": 0.28,
              "line-width": 12,
              "line-blur": 3
            }}
          />
          <Layer
            id="activity-route-line"
            type="line"
            paint={{
              "line-color": "#fc4c02",
              "line-opacity": 0.98,
              "line-width": 4
            }}
          />
        </Source>
      </Map>
      {!mapReady && <RoutePreview route={route} />}
      {styleWarning && <p className="mapWarning">{styleWarning}</p>}
    </div>
  );
}

function RoutePreview({ route }: { route: RouteGeoJson }) {
  const points = route.geometry.coordinates;
  const bounds = routeBounds(points);
  const width = Math.max(bounds.maxLng - bounds.minLng, 0.0001);
  const height = Math.max(bounds.maxLat - bounds.minLat, 0.0001);
  const path = points
    .map(([lng, lat], index) => {
      const x = 24 + ((lng - bounds.minLng) / width) * 252;
      const y = 24 + ((bounds.maxLat - lat) / height) * 172;
      return `${index === 0 ? "M" : "L"} ${x.toFixed(1)} ${y.toFixed(1)}`;
    })
    .join(" ");

  return (
    <svg className="routePreview" viewBox="0 0 300 220" role="img" aria-label="Route preview">
      <path className="routePreviewGlow" d={path} />
      <path className="routePreviewLine" d={path} />
    </svg>
  );
}

function AdvicePanel({
  advice,
  generateError,
  isGenerating,
  onGenerateAdvice
}: {
  advice: TrainingAdvice[];
  generateError: Error | null;
  isGenerating: boolean;
  onGenerateAdvice: () => void;
}) {
  const latest = advice.find((item) => item.activity_id == null);
  const [draft, setDraft] = useState("");
  const [messages, setMessages] = useState<AdviceChatMessage[]>([]);
  const chat = useMutation({
    mutationFn: ({ id, nextMessages }: { id: number; nextMessages: AdviceChatMessage[] }) =>
      api.chatAdvice(id, nextMessages),
    onSuccess: (response) => {
      setMessages((current) => [...current, { role: "assistant", content: response.message }]);
    }
  });

  useEffect(() => {
    setDraft("");
    setMessages([]);
  }, [latest?.id]);

  function sendFollowUp(event: React.FormEvent) {
    event.preventDefault();
    const content = draft.trim();
    if (!latest || !content || chat.isPending) {
      return;
    }

    const nextMessages: AdviceChatMessage[] = [...messages, { role: "user", content }];
    setDraft("");
    setMessages(nextMessages);
    chat.mutate({ id: latest.id, nextMessages });
  }

  return (
    <section className="panel">
      <div className="panelHeader">
        <div>
          <h2>Training Advice</h2>
          <p className="muted">Recent training</p>
        </div>
        <button onClick={onGenerateAdvice} disabled={isGenerating}>
          <Sparkles size={16} /> {isGenerating ? "Generating..." : "Generate"}
        </button>
      </div>
      {generateError && isGenerating && <p className="error">{generateError.message}</p>}
      {!latest && <p className="muted">No advice generated yet.</p>}
      {latest && (
        <>
          <AdviceCard advice={latest} />
          <div className="coachChat">
            <h3>Follow-up Chat</h3>
            <div className="chatTranscript">
              {messages.length === 0 && (
                <p className="muted">Ask a follow-up about pacing, recovery, schedule changes, or how to adapt the advice.</p>
              )}
              {messages.map((message, index) => (
                <div key={`${message.role}-${index}`} className={`chatBubble ${message.role}`}>
                  {message.content}
                </div>
              ))}
              {chat.isPending && <div className="chatBubble assistant">Thinking...</div>}
            </div>
            {chat.error && <p className="error">{(chat.error as Error).message}</p>}
            <form className="chatForm" onSubmit={sendFollowUp}>
              <input
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                placeholder="Ask a follow-up..."
              />
              <button type="submit" disabled={chat.isPending || draft.trim().length === 0}>
                Send
              </button>
            </form>
          </div>
        </>
      )}
    </section>
  );
}

function AdviceCard({ advice }: { advice: TrainingAdvice }) {
  return (
    <div className="advice">
      <p className="coachNote">{advice.body.summary}</p>
      {advice.body.load_observations.length > 0 && (
        <div className="adviceSection">
          <strong>What I see</strong>
          <ul>
            {advice.body.load_observations.map((item) => <li key={item}>{item}</li>)}
          </ul>
        </div>
      )}
      {advice.body.risks.length > 0 && (
        <div className="adviceSection">
          <strong>Watch-outs</strong>
          <ul>
            {advice.body.risks.map((item) => <li key={item}>{item}</li>)}
          </ul>
        </div>
      )}
      <div className="adviceSection">
        <strong>Next 7 days</strong>
        <ul>
          {advice.body.next_7_days.map((item) => <li key={item}>{item}</li>)}
        </ul>
      </div>
      <div className="adviceSection">
        <strong>Recovery</strong>
        <p>{advice.body.recovery_notes}</p>
      </div>
    </div>
  );
}

function metersToMiles(value?: number | null) {
  return ((value ?? 0) / 1609.344).toFixed(1);
}

function secondsToTime(value?: number | null) {
  const total = value ?? 0;
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  return hours > 0 ? `${hours}h ${minutes}m` : `${minutes}m`;
}

function pacePerMile(distanceMeters?: number | null, movingSeconds?: number | null) {
  if (!distanceMeters || !movingSeconds) {
    return "n/a";
  }

  const miles = distanceMeters / 1609.344;
  if (miles <= 0) {
    return "n/a";
  }

  const secondsPerMile = Math.round(movingSeconds / miles);
  const minutes = Math.floor(secondsPerMile / 60);
  const seconds = (secondsPerMile % 60).toString().padStart(2, "0");
  return `${minutes}:${seconds} /mi`;
}

function formatNumber(value?: number | null) {
  return value == null ? "n/a" : value.toFixed(0);
}

function formatCadence(activity: ActivityDetail) {
  if (activity.average_cadence == null) {
    return "n/a";
  }

  const cadence = activity.sport_type?.toLowerCase() === "run"
    ? activity.average_cadence * 2
    : activity.average_cadence;
  return `${cadence.toFixed(0)} spm`;
}

function toDateInputValue(value?: string | null) {
  if (!value) {
    return "";
  }

  const match = value.match(/^\d{4}-\d{2}-\d{2}/);
  return match?.[0] ?? "";
}

function mapTilerStyleUrl(key?: string) {
  const value = envValue(key);
  return value ? `https://api.maptiler.com/maps/outdoor-v2/style.json?key=${value}` : null;
}

function stadiaStyleUrl(key?: string) {
  const value = envValue(key);
  return value ? `https://tiles.stadiamaps.com/styles/outdoors.json?api_key=${value}` : null;
}

function envValue(value?: string) {
  const trimmed = value?.trim();
  return trimmed ? trimmed : null;
}

function activityRouteGeoJson(activity: ActivityDetail): RouteGeoJson | null {
  const coordinates = activityCoordinates(activity);
  if (coordinates.length < 2) {
    return null;
  }

  return {
    type: "Feature",
    properties: {},
    geometry: {
      type: "LineString",
      coordinates
    }
  };
}

function activityCoordinates(activity: ActivityDetail): [number, number][] {
  const raw = parseRawActivity(activity.raw_activity_json);
  const summaryPolyline = raw?.map?.summary_polyline ?? raw?.map?.polyline;
  if (typeof summaryPolyline === "string" && summaryPolyline.length > 0) {
    return decodePolyline(summaryPolyline).map(([lat, lng]) => [lng, lat]);
  }

  const startLatLng = raw?.start_latlng;
  const endLatLng = raw?.end_latlng;
  if (isLatLng(startLatLng) && isLatLng(endLatLng)) {
    return [
      [startLatLng[1], startLatLng[0]],
      [endLatLng[1], endLatLng[0]]
    ];
  }

  return [];
}

function parseRawActivity(rawActivityJson: string): any {
  try {
    return JSON.parse(rawActivityJson);
  } catch {
    return null;
  }
}

function isLatLng(value: unknown): value is [number, number] {
  return Array.isArray(value)
    && value.length === 2
    && typeof value[0] === "number"
    && typeof value[1] === "number";
}

function decodePolyline(polyline: string): [number, number][] {
  const coordinates: [number, number][] = [];
  let index = 0;
  let lat = 0;
  let lng = 0;

  while (index < polyline.length) {
    const latResult = decodePolylineValue(polyline, index);
    index = latResult.nextIndex;
    lat += latResult.value;

    const lngResult = decodePolylineValue(polyline, index);
    index = lngResult.nextIndex;
    lng += lngResult.value;

    coordinates.push([lat / 1e5, lng / 1e5]);
  }

  return coordinates;
}

function decodePolylineValue(polyline: string, startIndex: number) {
  let result = 0;
  let shift = 0;
  let index = startIndex;
  let byte = 0;

  do {
    byte = polyline.charCodeAt(index++) - 63;
    result |= (byte & 0x1f) << shift;
    shift += 5;
  } while (byte >= 0x20 && index < polyline.length);

  return {
    value: result & 1 ? ~(result >> 1) : result >> 1,
    nextIndex: index
  };
}

function routeBounds(coordinates: [number, number][]) {
  return coordinates.reduce(
    (bounds, [lng, lat]) => ({
      minLng: Math.min(bounds.minLng, lng),
      maxLng: Math.max(bounds.maxLng, lng),
      minLat: Math.min(bounds.minLat, lat),
      maxLat: Math.max(bounds.maxLat, lat)
    }),
    {
      minLng: coordinates[0][0],
      maxLng: coordinates[0][0],
      minLat: coordinates[0][1],
      maxLat: coordinates[0][1]
    }
  );
}
