import { useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import Map, { Layer, Source } from "react-map-gl/maplibre";
import type { StyleSpecification } from "maplibre-gl";
import {
  Activity,
  CalendarDays,
  ChevronLeft,
  ChevronRight,
  CheckCircle2,
  ClipboardList,
  LogOut,
  Eye,
  EyeOff,
  ArrowRight,
  RefreshCw,
  Save,
  Settings2,
  ShieldCheck,
  Sparkles,
  Target,
  Footprints,
  Clock3,
  Route,
  Unplug,
  XCircle
} from "lucide-react";
import {
  Line,
  LineChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis
} from "recharts";
import {
  addMonths,
  eachDayOfInterval,
  endOfMonth,
  endOfWeek,
  format,
  isSameMonth,
  parseISO,
  startOfMonth,
  startOfWeek,
  subMonths
} from "date-fns";
import {
  api,
  ApiError,
  ActivityDetail,
  ActivitySummary,
  MeResponse,
  StravaStatus,
  TrainingAdvice,
  type AdviceChatTurn
} from "./lib/api";

const CONFIGURED_MAP_STYLE_URL = envValue(import.meta.env.VITE_MAP_STYLE_URL);

const FALLBACK_MAP_STYLE: StyleSpecification = {
  version: 8,
  sources: {},
  layers: [
    {
      id: "route-background",
      type: "background",
      paint: {
        "background-color": "#e8eee9"
      }
    }
  ]
};

const ROUTE_MAP_PADDING = 12;
const ROUTE_MAP_MAX_ZOOM = 13;

type RouteGeoJson = {
  type: "Feature";
  properties: Record<string, never>;
  geometry: {
    type: "LineString";
    coordinates: [number, number][];
  };
};

// A generation that reached the LLM is stored server-side even when this client
// never sees the response (dropped connection, closed laptop). Rather than
// report a failure the runner would only fix by paying for a second
// generation, poll the advice list for the row the server is still writing.
const ADVICE_RECOVERY_TIMEOUT_MS = 120_000;
const ADVICE_RECOVERY_INTERVAL_MS = 3_000;

type AdviceRecovery = { activityId: number | null };

/// Identity of a stored entry: regenerating today's overview rewrites its row
/// in place, so the id alone would not show that the server produced something
/// new. created_at moves on every write.
function adviceSignature(item: TrainingAdvice) {
  return `${item.id}:${item.created_at}`;
}

function isAdviceFor(item: TrainingAdvice, activityId: number | null) {
  return activityId === null ? item.activity_id == null : item.activity_id === activityId;
}

async function waitForPersistedAdvice(
  activityId: number | null,
  known: Set<string>,
  token: { cancelled: boolean }
): Promise<TrainingAdvice[] | null> {
  const deadline = Date.now() + ADVICE_RECOVERY_TIMEOUT_MS;
  while (Date.now() < deadline && !token.cancelled) {
    await new Promise((resolve) => setTimeout(resolve, ADVICE_RECOVERY_INTERVAL_MS));
    if (token.cancelled) {
      return null;
    }
    try {
      const list = await api.advice();
      const landed = list.some(
        (item) => isAdviceFor(item, activityId) && !known.has(adviceSignature(item))
      );
      if (landed) {
        return list;
      }
    } catch {
      // The API may still be unreachable; keep waiting until the deadline.
    }
  }
  return null;
}

type SyncWatchState = {
  lastCompletedSyncAt?: string | null;
  failedJobs: number;
  sawActiveJob: boolean;
};

export function App() {
  const queryClient = useQueryClient();
  const [sessionEnded, setSessionEnded] = useState(false);
  const me = useQuery({ queryKey: ["me"], queryFn: api.me, retry: false });
  const authenticated = me.data?.authenticated === true;
  const wasAuthenticated = useRef(false);

  const clearSession = async () => {
    await queryClient.cancelQueries();
    queryClient.removeQueries({ predicate: (query) => query.queryKey[0] !== "me" });
    queryClient.getMutationCache().clear();
    queryClient.setQueryData<MeResponse>(["me"], { authenticated: false });
  };

  useEffect(() => {
    if (!authenticated) return;
    let ending = false;
    const onError = (error: unknown) => {
      if (ending || !(error instanceof ApiError) || error.status !== 401) return;
      ending = true;
      setSessionEnded(true);
      void clearSession();
    };
    const queries = queryClient.getQueryCache().subscribe((event) => {
      if (event.type === "updated" && event.action.type === "error") onError(event.query.state.error);
    });
    const mutations = queryClient.getMutationCache().subscribe((event) => {
      if (event.type === "updated" && event.action.type === "error") onError(event.mutation.state.error);
    });
    return () => { queries(); mutations(); };
  }, [authenticated, queryClient]);

  useEffect(() => {
    if (wasAuthenticated.current && me.data?.authenticated === false) {
      setSessionEnded(true);
      void clearSession();
    }
    wasAuthenticated.current = authenticated;
  }, [authenticated, me.data, queryClient]);

  if (me.isPending) return <Shell><section className="sessionState" role="status"><RefreshCw className="loadingIcon" /><h1>Getting things ready</h1><p className="muted">Checking your session…</p></section></Shell>;
  if (me.isError) return <Shell><section className="sessionState" role="alert"><ShieldCheck size={32} /><h1>Can’t reach RunningBuddy</h1><p className="muted">Check your connection, then try again.</p><button onClick={() => me.refetch()} disabled={me.isFetching}>{me.isFetching ? "Checking…" : "Try again"}</button></section></Shell>;
  if (!authenticated) return <Login sessionEnded={sessionEnded} onLogin={() => setSessionEnded(false)} />;
  return <Dashboard me={me.data} onLogout={async () => { wasAuthenticated.current = false; setSessionEnded(false); await clearSession(); }} />;
}

function Dashboard({ me, onLogout }: { me: MeResponse; onLogout: () => Promise<void> }) {
  const queryClient = useQueryClient();
  const [selectedActivityId, setSelectedActivityId] = useState<number | null>(null);
  const [showAthleteConfig, setShowAthleteConfig] = useState(false);
  const [syncWatch, setSyncWatch] = useState<SyncWatchState | null>(null);
  const activities = useQuery({
    queryKey: ["activities"],
    queryFn: api.activities,
    enabled: me?.authenticated === true
  });
  const advice = useQuery({
    queryKey: ["advice"],
    queryFn: api.advice,
    enabled: me?.authenticated === true
  });
  const stravaStatus = useQuery({
    queryKey: ["stravaStatus"],
    queryFn: api.stravaStatus,
    enabled: me?.authenticated === true,
    refetchInterval: (query) => {
      const status = query.state.data as StravaStatus | undefined;
      return syncWatch || hasActiveSync(status) ? 2000 : false;
    }
  });
  const selectedActivity = useQuery({
    queryKey: ["activity", selectedActivityId],
    queryFn: () => api.activity(selectedActivityId!),
    enabled: me.authenticated && selectedActivityId !== null
  });
  const [adviceRecovery, setAdviceRecovery] = useState<AdviceRecovery | null>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (adviceRecoveryRef.current) adviceRecoveryRef.current.cancelled = true;
    };
  }, []);
  const adviceRecoveryRef = useRef<{ cancelled: boolean } | null>(null);
  const generateAdvice = useMutation<TrainingAdvice, Error, number | null, { known: Set<string> }>({
    mutationFn: (activityId: number | null) => api.generateAdvice(28, activityId),
    onMutate: () => {
      // A retry supersedes any recovery still polling for the previous attempt.
      if (adviceRecoveryRef.current) {
        adviceRecoveryRef.current.cancelled = true;
        adviceRecoveryRef.current = null;
      }
      setAdviceRecovery(null);
      const current = queryClient.getQueryData<TrainingAdvice[]>(["advice"]) ?? [];
      return { known: new Set(current.map(adviceSignature)) };
    },
    onError: async (_error, activityId, context) => {
      if (!mounted.current || (_error instanceof ApiError && _error.status === 401)) return;
      // The request died, but the server keeps going: wait for its row instead
      // of throwing away a generation that was already paid for.
      const token = { cancelled: false };
      adviceRecoveryRef.current = token;
      setAdviceRecovery({ activityId });
      const list = await waitForPersistedAdvice(activityId, context?.known ?? new Set(), token);
      if (token.cancelled) {
        return;
      }
      adviceRecoveryRef.current = null;
      setAdviceRecovery(null);
      if (list) {
        queryClient.setQueryData<TrainingAdvice[]>(["advice"], list);
        queryClient.invalidateQueries({ queryKey: ["advice-calendar"] });
        queryClient.invalidateQueries({ queryKey: ["advice-day"] });
        generateAdvice.reset();
      }
    },
    onSuccess: (newAdvice) => {
      if (!mounted.current) return;
      queryClient.setQueryData<TrainingAdvice[]>(["advice"], (current = []) => [
        newAdvice,
        ...current.filter((item) => item.id !== newAdvice.id)
      ]);
      // A new overview may add a day to the calendar or replace today's entry.
      queryClient.invalidateQueries({ queryKey: ["advice-calendar"] });
      queryClient.invalidateQueries({ queryKey: ["advice-day"] });
    }
  });
  const logout = useMutation({
    mutationFn: api.logout,
    onSuccess: onLogout
  });

  // The server queues a catch-up sync on the first visit of each day; watch it
  // the same way we watch a manually triggered sync.
  const autoSyncWatchedRef = useRef(false);
  useEffect(() => {
    if (!me?.auto_sync_started || autoSyncWatchedRef.current) {
      return;
    }
    autoSyncWatchedRef.current = true;
    setSyncWatch({
      lastCompletedSyncAt: stravaStatus.data?.last_completed_sync_at,
      failedJobs: stravaStatus.data?.failed_jobs ?? 0,
      sawActiveJob: false
    });
  }, [me?.auto_sync_started, stravaStatus.data]);

  const lastCompletedSyncAtRef = useRef<string | null | undefined>(undefined);
  useEffect(() => {
    const current = stravaStatus.data?.last_completed_sync_at;
    if (current === undefined) {
      return;
    }
    if (lastCompletedSyncAtRef.current === undefined) {
      lastCompletedSyncAtRef.current = current;
      return;
    }
    if (lastCompletedSyncAtRef.current !== current) {
      lastCompletedSyncAtRef.current = current;
      queryClient.invalidateQueries({ queryKey: ["activities"] });
      queryClient.invalidateQueries({ queryKey: ["activity"] });
      queryClient.invalidateQueries({ queryKey: ["advice"] });
    }
  }, [queryClient, stravaStatus.data?.last_completed_sync_at]);

  useEffect(() => {
    if (!syncWatch || !stravaStatus.data) {
      return;
    }

    if (hasActiveSync(stravaStatus.data)) {
      if (!syncWatch.sawActiveJob) {
        setSyncWatch({ ...syncWatch, sawActiveJob: true });
      }
      return;
    }

    const syncFinished = syncWatch.sawActiveJob
      || stravaStatus.data.last_completed_sync_at !== syncWatch.lastCompletedSyncAt
      || stravaStatus.data.failed_jobs > syncWatch.failedJobs;

    if (!syncFinished) {
      return;
    }

    setSyncWatch(null);
    queryClient.invalidateQueries({ queryKey: ["activities"] });
    queryClient.invalidateQueries({ queryKey: ["activity"] });
    queryClient.invalidateQueries({ queryKey: ["advice"] });
    queryClient.invalidateQueries({ queryKey: ["stravaStatus"] });
  }, [queryClient, stravaStatus.data, syncWatch]);

  const isSyncing = Boolean(syncWatch) || hasActiveSync(stravaStatus.data);
  const watchSync = () =>
    setSyncWatch({
      lastCompletedSyncAt: stravaStatus.data?.last_completed_sync_at,
      failedJobs: stravaStatus.data?.failed_jobs ?? 0,
      sawActiveJob: false
    });

  return (
    <Shell
      action={
        <div className="navActions">
          <button
            type="button"
            onClick={() => setShowAthleteConfig((current) => !current)}
            aria-expanded={showAthleteConfig}
          >
            <Settings2 size={16} /> Configure Athlete
          </button>
          <span className="signedInUser">{me.username}</span>
          <button className="iconButton" disabled={logout.isPending} onClick={() => logout.mutate()} aria-label="Log out" title="Log out">
            <LogOut size={18} />
          </button>
        </div>
      }
    >
      <header className="dashboardHeading">
        <div><p className="eyebrow">YOUR TRAINING, IN VIEW</p><h1>Every run adds up.</h1><p className="muted">Review your progress and make room for what’s next.</p></div>
        <div className="dashboardMeta"><span className="dateBadge"><CalendarDays size={16} />{format(new Date(), "EEEE, MMM d")}</span><div className="sectionLinks"><a href="#activities">Activities <ArrowRight size={14} /></a><a href="#training-overview">Training overview <ArrowRight size={14} /></a></div></div>
      </header>
      {logout.error && <p className="error notice" role="alert">Couldn’t log out. Check your connection and try again.</p>}
      {activities.isError && <div className="error notice" role="alert">Couldn’t load activities. <button onClick={() => activities.refetch()}>Try again</button></div>}
      {/* Collapsed view carries only what gets checked daily: when the last
          sync landed, and a button to run another. Everything else about the
          connection lives behind Configure Athlete. */}
      <SyncBar
        status={stravaStatus.data}
        isSyncing={isSyncing}
        onSyncQueued={watchSync}
      />

      {showAthleteConfig && (
        <section className="configPanel" aria-label="Athlete configuration">
          <div className="sectionHeading"><span className="sectionIcon"><Settings2 size={20} /></span><div><p className="eyebrow">MAKE IT YOURS</p><h2>Athlete Configuration</h2></div></div>
          <StravaStatusPanel
            status={stravaStatus.data}
            isLoading={stravaStatus.isLoading}
            isLive={isSyncing}
            actions={<StravaActions isSyncing={isSyncing} onSyncQueued={watchSync} />}
          />
          <TrainingPlanPanel me={me} />
        </section>
      )}

      <DashboardSummary activities={activities.data ?? []} />
      <TrendChart activities={activities.data ?? []} />

      <main className="layout" id="activities">
        <ActivityTable
          activities={activities.data ?? []}
          isLoading={activities.isLoading}
          selectedId={selectedActivityId}
          onSelect={setSelectedActivityId}
        />
        <ActivityDetailPanel
          key={selectedActivityId ?? "no-activity"}
          activity={selectedActivity.data}
          advice={selectedActivityId ? advice.data?.find((item) => item.activity_id === selectedActivityId) : undefined}
          generateError={generateAdvice.variables === selectedActivityId ? generateAdvice.error : null}
          isGenerating={generateAdvice.isPending && generateAdvice.variables === selectedActivityId}
          isRecovering={adviceRecovery?.activityId === selectedActivityId}
          isLoading={selectedActivity.isLoading}
          selectedActivityId={selectedActivityId}
          onGenerateAdvice={() => generateAdvice.mutate(selectedActivityId)}
        />
      </main>

      <AdvicePanel
        generateError={generateAdvice.variables == null ? generateAdvice.error : null}
        isGenerating={generateAdvice.isPending && generateAdvice.variables == null}
        isRecovering={adviceRecovery != null && adviceRecovery.activityId == null}
        onGenerateAdvice={() => generateAdvice.mutate(null)}
      />
    </Shell>
  );
}

function Login({ sessionEnded, onLogin }: { sessionEnded: boolean; onLogin: () => void }) {
  const queryClient = useQueryClient();
  const [username, setUsername] = useState("admin");
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const login = useMutation({
    mutationFn: () => api.login(username.trim(), password),
    onSuccess: (data) => { setPassword(""); onLogin(); queryClient.setQueryData(["me"], data); }
  });

  return (
    <Shell>
      <main className="loginLayout">
        <section className="loginStory">
          <span className="eyebrow">A LITTLE PERSPECTIVE. EVERY RUN.</span>
          <h1>Your miles.<br />Your momentum.</h1>
          <p>A home for your training, with the bigger picture always in view.</p>
          <div className="loginFeatures">
            <div><Activity size={22} /><span><strong>See your progress</strong><small>Your Strava activities, together in one place.</small></span></div>
            <div><Target size={22} /><span><strong>Stay focused</strong><small>Keep your training plan and goals close.</small></span></div>
            <div><Sparkles size={22} /><span><strong>Reflect on each run</strong><small>Training insights grounded in your activity.</small></span></div>
          </div>
        </section>
        <form className="login" onSubmit={(event) => { event.preventDefault(); if (!login.isPending && username.trim() && password) login.mutate(); }} aria-labelledby="login-title" aria-busy={login.isPending}>
          <div className="loginIcon"><ShieldCheck size={26} /></div>
          <div><h2 id="login-title">Welcome back</h2><p className="muted">Log in to your training space.</p></div>
          {sessionEnded && <p className="notice" role="status">Your session has ended. Please log in again.</p>}
          <label htmlFor="username">Username</label>
          <input id="username" name="username" disabled={login.isPending} autoComplete="username" autoCapitalize="none" spellCheck={false} required value={username} onChange={(event) => { setUsername(event.target.value); login.reset(); }} />
          <label htmlFor="password">Password</label>
          <div className="passwordField">
            <input id="password" name="password" disabled={login.isPending} autoComplete="current-password" type={showPassword ? "text" : "password"} required value={password} aria-describedby={login.error ? "login-error" : undefined} onChange={(event) => { setPassword(event.target.value); login.reset(); }} />
            <button type="button" className="passwordToggle" aria-label={showPassword ? "Hide password" : "Show password"} aria-pressed={showPassword} onClick={() => setShowPassword(!showPassword)}>{showPassword ? <EyeOff size={18} /> : <Eye size={18} />}</button>
          </div>
          {login.error && <p id="login-error" className="error notice" role="alert">{login.error instanceof ApiError && login.error.status === 401 ? "Incorrect username or password. Try again." : "Couldn’t log in. Check your connection and try again."}</p>}
          <button className="loginSubmit" type="submit" disabled={login.isPending || !username.trim() || !password}>{login.isPending ? <><span className="buttonSpinner" aria-hidden="true" />Logging in…</> : <>Log in <ArrowRight size={18} /></>}</button>
          <p className="loginHint">Use your RunningBuddy account to continue.</p>
        </form>
      </main>
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

/// SQLite stamps `CURRENT_TIMESTAMP` in UTC with no zone marker. parseISO reads
/// a bare "yyyy-MM-dd HH:mm:ss" as local time, which showed every sync hours
/// off, so pin it to UTC and let the browser render it in the runner's zone.
function formatServerTime(value: string) {
  const withT = value.includes("T") ? value : value.replace(" ", "T");
  const zoned = /([Zz]|[+-]\d{2}:?\d{2})$/.test(withT) ? withT : `${withT}Z`;
  return format(parseISO(zoned), "MMM d, h:mm a");
}

/// The collapsed view: when the last sync finished, and one button to run
/// another. Connection details sit behind Configure Athlete.
function SyncBar({
  status,
  isSyncing,
  onSyncQueued
}: {
  status?: StravaStatus;
  isSyncing: boolean;
  onSyncQueued: () => void;
}) {
  return (
    <section className="syncBar" aria-label="Strava sync">
      <span className="muted">
        {isSyncing
          ? "Syncing with Strava..."
          : status?.last_completed_sync_at
            ? `Last sync ${formatServerTime(status.last_completed_sync_at)}`
            : "No completed sync yet"}
      </span>
      <SyncButton isSyncing={isSyncing} onSyncQueued={onSyncQueued} />
    </section>
  );
}

function SyncButton({
  isSyncing,
  onSyncQueued
}: {
  isSyncing: boolean;
  onSyncQueued: () => void;
}) {
  const queryClient = useQueryClient();
  const sync = useMutation({
    mutationFn: api.sync,
    onSuccess: () => {
      onSyncQueued();
      queryClient.invalidateQueries({ queryKey: ["stravaStatus"] });
    }
  });
  // Stays disabled for the whole run, not just the queueing request: the work
  // happens in the background worker afterwards.
  const busy = sync.isPending || isSyncing;

  return (
    <button onClick={() => sync.mutate()} disabled={busy}>
      {busy ? <span className="buttonSpinner" aria-hidden="true" /> : <RefreshCw size={16} />}
      {busy ? "Syncing..." : "Sync"}
    </button>
  );
}

function StravaActions({
  isSyncing,
  onSyncQueued
}: {
  isSyncing: boolean;
  onSyncQueued: () => void;
}) {
  const connect = useMutation({
    mutationFn: api.stravaConnect,
    onSuccess: (data) => {
      window.location.href = data.authorization_url;
    }
  });

  return (
    <div className="toolbar">
      <button onClick={() => connect.mutate()} disabled={connect.isPending}>
        <Unplug size={16} /> Connect Strava
      </button>
      <SyncButton isSyncing={isSyncing} onSyncQueued={onSyncQueued} />
    </div>
  );
}

function StravaStatusPanel({
  status,
  isLoading,
  isLive,
  actions
}: {
  status?: StravaStatus;
  isLoading: boolean;
  isLive: boolean;
  actions?: React.ReactNode;
}) {
  const athleteName = [status?.athlete?.firstname, status?.athlete?.lastname].filter(Boolean).join(" ");
  const isConnected = status?.connected === true;
  const activeSyncCount = (status?.queued_jobs ?? 0) + (status?.running_jobs ?? 0);
  const syncLabel = activeSyncCount > 0
    ? `${activeSyncCount} sync ${activeSyncCount === 1 ? "job" : "jobs"} active`
    : isLive
      ? "Watching sync"
      : status?.last_completed_sync_at
        ? `Last sync ${formatServerTime(status.last_completed_sync_at)}`
        : "No completed sync yet";

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
        <span className={isLive ? "liveSyncStatus" : undefined}>
          {isLive && <RefreshCw size={14} />}
          {syncLabel}
        </span>
        <span>{status ? `${status.queued_jobs} queued / ${status.running_jobs} running / ${status.failed_jobs} failed` : "Queue unavailable"}</span>
      </div>
      {actions}
    </section>
  );
}

function hasActiveSync(status?: StravaStatus) {
  return Boolean(status && (status.queued_jobs > 0 || status.running_jobs > 0));
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
    <section className="metrics" aria-label="All synced activity totals">
      <Metric icon={<Activity size={18} />} label="Activities" value={totals.runs.toString()} />
      <Metric icon={<Route size={18} />} label="Miles" value={metersToMiles(totals.distance)} />
      <Metric icon={<Clock3 size={18} />} label="Moving Time" value={secondsToTime(totals.time)} />
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

// Matches Run, TrailRun, VirtualRun, etc. — mirrors is_run_sport in src/advice.rs.
function isRunSport(sportType?: string | null) {
  return sportType?.toLowerCase().includes("run") ?? false;
}

function TrendChart({ activities }: { activities: ActivitySummary[] }) {
  const data = activities
    .filter(
      (activity) => activity.start_date && activity.distance_meters && isRunSport(activity.sport_type)
    )
    .slice()
    .reverse()
    .map((activity) => ({
      date: format(parseISO(activity.start_date!), "MMM d"),
      miles: Number(metersToMiles(activity.distance_meters))
    }));

  return (
    <section className="panel trendPanel">
      <div className="sectionHeading"><span className="sectionIcon"><Activity size={20} /></span><div><h2>Run Distance Trend</h2><p className="sectionSubtitle">Your running distance, one activity at a time.</p></div><span className="chartLegend"><i />Miles per run</span></div>
      {data.length === 0 && <p className="muted">No synced runs yet.</p>}
      {data.length > 0 && <div className="chart">
        <ResponsiveContainer>
          <LineChart data={data}>
            <CartesianGrid vertical={false} stroke="#e9efe9" strokeDasharray="3 4" />
            <XAxis dataKey="date" minTickGap={28} tickLine={false} axisLine={{ stroke: "#d7e2d9" }} />
            <YAxis width={42} tickLine={false} axisLine={false} />
            <Tooltip />
            <Line type="monotone" dataKey="miles" stroke="#006d77" strokeWidth={2} dot={false} isAnimationActive={false} />
          </LineChart>
        </ResponsiveContainer>
      </div>}
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
    <section className="panel activityListPanel">
      <div className="sectionHeading"><span className="sectionIcon"><Footprints size={20} /></span><div><h2>Activities</h2><p className="sectionSubtitle">Small steps. A bigger picture.</p></div><span className="countBadge">{activities.length}</span></div>
      {isLoading && <p className="muted">Loading activities...</p>}
      {!isLoading && activities.length === 0 && <p className="muted">No synced activities yet.</p>}
      <div className="table">
        {activities.map((activity) => (
          <button
            key={activity.id}
            className={activity.id === selectedId ? "row selected" : "row"}
            onClick={() => onSelect(activity.id)}
            aria-pressed={activity.id === selectedId}
          >
            <span className="activityRowIcon"><Activity size={18} /></span>
            <span className="activityRowTitle"><strong>{activity.name}</strong><small>{activity.start_date ? format(parseISO(activity.start_date), "MMM d") : "n/a"}{activity.sport_type ? ` · ${activity.sport_type}` : ""}</small></span>
            <span className="activityRowDistance">{metersToMiles(activity.distance_meters)} <small>mi</small></span>
            <ChevronRight size={16} className="rowArrow" />
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
  isRecovering,
  isLoading,
  selectedActivityId,
  onGenerateAdvice
}: {
  activity?: ActivityDetail;
  advice?: TrainingAdvice;
  generateError: Error | null;
  isGenerating: boolean;
  isRecovering: boolean;
  isLoading: boolean;
  selectedActivityId: number | null;
  onGenerateAdvice: () => void;
}) {
  const [view, setView] = useState<"details" | "advice">("details");

  return (
    <section className="panel detailPanel">
      <div className="panelHeader">
        <div><p className="eyebrow">THE DETAILS</p><h2>Activity Detail</h2></div>
        <button onClick={() => { setView("advice"); onGenerateAdvice(); }} disabled={!selectedActivityId || isGenerating || isRecovering}>
          <AdviceButtonContent
            isGenerating={isGenerating}
            isRecovering={isRecovering}
            label="Get Activity Advice"
          />
        </button>
      </div>
      {activity && (
        <div className="selectedActivityHeading">
          <strong>{activity.name}</strong>
          <span>{activity.sport_type ?? "Activity"} · {activity.start_date ? format(parseISO(activity.start_date), "MMM d, h:mm a") : "Date unavailable"}</span>
        </div>
      )}
      {selectedActivityId !== null && (
        <div className="activityViewSwitch" role="group" aria-label="Activity view">
          <button type="button" aria-pressed={view === "details"} onClick={() => setView("details")}><Route size={16} />Details</button>
          <button type="button" aria-pressed={view === "advice"} onClick={() => setView("advice")}><Sparkles size={16} />Advice{advice && <span className="savedAdviceDot" aria-label="Saved advice available" />}</button>
        </div>
      )}
      <div className="activityDetailBody" key={view} role="region" aria-label={view === "details" ? "Activity details" : "Activity advice"} tabIndex={0}>
        <AdviceRecoveryNotice isRecovering={isRecovering} generateError={generateError} />
        {isLoading && <p className="muted">Loading activity...</p>}
        {!selectedActivityId && <div className="emptyState"><span className="emptyStateIcon"><Route size={32} /></span><h3>Every activity has a story.</h3><p>Select an activity to explore your route, pace, and training insights.</p></div>}
        {activity && view === "details" && (
          <>
            <dl className="details">
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
          </>
        )}
        {activity && view === "advice" && (
            <div className="activityAdvice">
              <h3>Activity Advice</h3>
              {generateError && isGenerating && <p className="error">{generateError.message}</p>}
              {!advice && <p className="muted">No advice generated for this activity yet.</p>}
              {advice && <AdviceCard advice={advice} />}
            </div>
        )}
      </div>
    </section>
  );
}

function ActivityMap({ activity }: { activity: ActivityDetail }) {
  const route = useMemo(() => activityRouteGeoJson(activity), [activity]);
  const [mapStyle, setMapStyle] = useState<string | StyleSpecification>(
    CONFIGURED_MAP_STYLE_URL ?? FALLBACK_MAP_STYLE
  );
  const [styleWarning, setStyleWarning] = useState<string | null>(null);

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
            maxZoom: ROUTE_MAP_MAX_ZOOM,
            padding: ROUTE_MAP_PADDING
          }
        }}
        key={typeof mapStyle === "string" ? mapStyle : "fallback-osm"}
        mapStyle={mapStyle}
        attributionControl={false}
        style={{ width: "100%", height: "100%" }}
        onError={() => {
          if (mapStyle !== FALLBACK_MAP_STYLE) {
            setMapStyle(FALLBACK_MAP_STYLE);
            setStyleWarning("Map style failed to load, using the local route view.");
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
      {styleWarning && <p className="mapWarning">{styleWarning}</p>}
    </div>
  );
}

const DAY_KEY = "yyyy-MM-dd";

/// Training overviews are kept one-per-day, so the panel is a date browser:
/// pick a day in the calendar and get that day's overview plus the follow-up
/// conversation that was saved with it.
function AdvicePanel({
  generateError,
  isGenerating,
  isRecovering,
  onGenerateAdvice
}: {
  generateError: Error | null;
  isGenerating: boolean;
  isRecovering: boolean;
  onGenerateAdvice: () => void;
}) {
  const queryClient = useQueryClient();
  const today = format(new Date(), DAY_KEY);
  const [pickedDate, setPickedDate] = useState<string | null>(null);
  const [monthCursor, setMonthCursor] = useState<Date | null>(null);
  const [draft, setDraft] = useState("");

  const calendar = useQuery({ queryKey: ["advice-calendar"], queryFn: api.adviceCalendar });
  const storedDates = useMemo(
    () =>
      new Set(
        (calendar.data ?? [])
          .map((entry) => entry.advice_date)
          .filter((date): date is string => Boolean(date))
      ),
    [calendar.data]
  );

  // Default to the newest stored day until the runner picks one themselves.
  const newestStoredDate = calendar.data?.[0]?.advice_date ?? null;
  const selectedDate = pickedDate ?? newestStoredDate ?? today;
  const visibleMonth = monthCursor ?? startOfMonth(parseISO(selectedDate));
  const hasStoredDay = storedDates.has(selectedDate);

  const day = useQuery({
    queryKey: ["advice-day", selectedDate],
    queryFn: () => api.adviceDay(selectedDate),
    enabled: hasStoredDay
  });

  const chat = useMutation({
    mutationFn: ({ id, message }: { id: number; message: string }) => api.chatAdvice(id, message),
    onSuccess: (response) => {
      queryClient.setQueryData(["advice-day", selectedDate], (current: typeof day.data) =>
        current ? { ...current, chat: response.chat } : current
      );
    }
  });

  useEffect(() => {
    setDraft("");
    chat.reset();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedDate]);

  const advice = day.data?.advice;
  const messages: AdviceChatTurn[] = day.data?.chat ?? [];

  function sendFollowUp(event: React.FormEvent) {
    event.preventDefault();
    const content = draft.trim();
    if (!advice || !content || chat.isPending) {
      return;
    }
    setDraft("");
    chat.mutate({ id: advice.id, message: content });
  }

  function generateForToday() {
    setPickedDate(today);
    setMonthCursor(startOfMonth(new Date()));
    onGenerateAdvice();
  }

  return (
    <section className="panel overviewPanel" id="training-overview">
      <div className="panelHeader">
        <div>
          <p className="eyebrow">REFLECT & LOOK AHEAD</p>
          <h2>Training Overview</h2>
          <p className="muted">
            {format(parseISO(selectedDate), "EEEE, MMM d, yyyy")}
            {selectedDate === today ? " · today" : ""}
          </p>
        </div>
        <button onClick={generateForToday} disabled={isGenerating || isRecovering}>
          <AdviceButtonContent
            isGenerating={isGenerating}
            isRecovering={isRecovering}
            label={storedDates.has(today) ? "Regenerate today" : "Generate"}
          />
        </button>
      </div>
      <div className="overviewLayout">
        <aside className="overviewSidebar" aria-label="Saved training overviews">
          <div className="sidebarHeading"><CalendarDays size={18} /><h3>Your training journal</h3></div>
          <p className="sectionSubtitle">Choose a saved day to revisit your advice and conversation.</p>
          <AdviceCalendar
            visibleMonth={visibleMonth}
            selectedDate={selectedDate}
            today={today}
            storedDates={storedDates}
            onSelect={setPickedDate}
            onMonthChange={setMonthCursor}
          />
        </aside>
        <div className="overviewContent">
          <AdviceRecoveryNotice isRecovering={isRecovering} generateError={generateError} />
          {day.error && <p className="error">{(day.error as Error).message}</p>}
          {!hasStoredDay && (
            <p className="muted">
              {selectedDate === today
                ? "No overview saved for today yet. Generate one to start the day's thread."
                : "No overview was saved for this day."}
            </p>
          )}
          {hasStoredDay && !advice && <p className="muted">Loading this day's overview...</p>}
          {advice && (
            <>
              <AdviceCard advice={advice} />
              <div className="coachChat">
                <h3>Follow-up Chat</h3>
                <div className="chatTranscript">
                  {messages.length === 0 && !chat.isPending && (
                    <p className="muted">Ask a follow-up about pacing, recovery, schedule changes, or how to adapt the advice.</p>
                  )}
                  {messages.map((message) => (
                    <div key={message.id} className={`chatBubble ${message.role}`}>
                      {message.content}
                    </div>
                  ))}
                  {chat.isPending && (
                    <>
                      <div className="chatBubble user">{chat.variables?.message}</div>
                      <div className="chatBubble assistant">Thinking...</div>
                    </>
                  )}
                </div>
                {chat.error && <p className="error">{(chat.error as Error).message}</p>}
                <form className="chatForm" onSubmit={sendFollowUp}>
                  <input
                    value={draft}
                    onChange={(event) => setDraft(event.target.value)}
                    placeholder="Ask a follow-up..."
                    aria-label="Ask a follow-up"
                  />
                  <button type="submit" disabled={chat.isPending || draft.trim().length === 0}>
                    Send
                  </button>
                </form>
              </div>
            </>
          )}
        </div>
      </div>
    </section>
  );
}

function AdviceCalendar({
  visibleMonth,
  selectedDate,
  today,
  storedDates,
  onSelect,
  onMonthChange
}: {
  visibleMonth: Date;
  selectedDate: string;
  today: string;
  storedDates: Set<string>;
  onSelect: (date: string) => void;
  onMonthChange: (month: Date) => void;
}) {
  const days = useMemo(
    () =>
      eachDayOfInterval({
        start: startOfWeek(startOfMonth(visibleMonth)),
        end: endOfWeek(endOfMonth(visibleMonth))
      }),
    [visibleMonth]
  );

  return (
    <div className="adviceCalendar">
      <div className="calendarHeader">
        <button
          type="button"
          className="calendarNav"
          aria-label="Previous month"
          onClick={() => onMonthChange(subMonths(visibleMonth, 1))}
        >
          <ChevronLeft size={16} />
        </button>
        <strong>{format(visibleMonth, "MMMM yyyy")}</strong>
        <button
          type="button"
          className="calendarNav"
          aria-label="Next month"
          onClick={() => onMonthChange(addMonths(visibleMonth, 1))}
        >
          <ChevronRight size={16} />
        </button>
      </div>
      <div className="calendarGrid" role="grid">
        {["S", "M", "T", "W", "T", "F", "S"].map((label, index) => (
          <span key={`${label}-${index}`} className="calendarWeekday">
            {label}
          </span>
        ))}
        {days.map((date) => {
          const key = format(date, DAY_KEY);
          const stored = storedDates.has(key);
          const classes = [
            "calendarDay",
            stored ? "hasAdvice" : "",
            key === selectedDate ? "selected" : "",
            key === today ? "today" : "",
            isSameMonth(date, visibleMonth) ? "" : "outsideMonth"
          ]
            .filter(Boolean)
            .join(" ");
          return (
            <button
              type="button"
              key={key}
              className={classes}
              aria-pressed={key === selectedDate}
              aria-label={`${format(date, "MMMM d, yyyy")}${stored ? " (overview saved)" : " (no overview)"}`}
              disabled={!stored && key !== today}
              onClick={() => onSelect(key)}
            >
              {format(date, "d")}
            </button>
          );
        })}
      </div>
      <p className="muted calendarHint">
        One overview is kept per day. Regenerating today rewrites today's and keeps its chat.
      </p>
    </div>
  );
}

function AdviceButtonContent({
  isGenerating,
  isRecovering = false,
  label
}: {
  isGenerating: boolean;
  isRecovering?: boolean;
  label: string;
}) {
  const busy = isGenerating || isRecovering;
  return (
    <>
      {busy ? <span className="buttonSpinner" aria-hidden="true" /> : <Sparkles size={16} />}
      {isRecovering ? "Recovering..." : isGenerating ? "Generating..." : label}
    </>
  );
}

/// While recovering, the request failed but the coach is probably still
/// working, so the failure is not news yet. Only report it once the recovery
/// window has closed without a stored result.
function AdviceRecoveryNotice({
  isRecovering,
  generateError
}: {
  isRecovering: boolean;
  generateError: Error | null;
}) {
  if (isRecovering) {
    return (
      <p className="muted">
        Lost the connection before the answer arrived. Checking whether the coach finished it
        anyway...
      </p>
    );
  }
  return generateError ? <p className="error">{generateError.message}</p> : null;
}

function AdviceCard({ advice }: { advice: TrainingAdvice }) {
  return (
    <div className="advice">
      <div className="coachSummary">
        <span className="summaryLabel"><Sparkles size={16} />The big picture</span>
        <p className="coachNote">{advice.body.summary}</p>
      </div>
      {(advice.body.priority_actions ?? []).length > 0 && (
        <div className="adviceSection">
          <strong>Priorities</strong>
          <ol>
            {advice.body.priority_actions!.map((item, index) => (
              <li key={index}>
                <strong>{item.action}</strong>
                <p>{item.reason}</p>
                <p><em>What’s uncertain:</em> {item.uncertainty}</p>
                <p><em>Adjust if:</em> {item.adjust_if}</p>
              </li>
            ))}
          </ol>
        </div>
      )}
      {advice.body.load_observations.length > 0 && (
        <div className="adviceSection">
          <strong>What I see</strong>
          <ul>
            {advice.body.load_observations.map((item) => <li key={item}>{item}</li>)}
          </ul>
        </div>
      )}
      {advice.body.risks.length > 0 && (
        <div className="adviceSection cautionSection">
          <strong>Watch-outs</strong>
          <ul>
            {advice.body.risks.map((item) => <li key={item}>{item}</li>)}
          </ul>
        </div>
      )}
      {(advice.body.last_7_days ?? []).length > 0 && (
        <div className="adviceSection">
          <strong>Last 7 days</strong>
          <ul>
            {advice.body.last_7_days.map((item) => <li key={item}>{item}</li>)}
          </ul>
        </div>
      )}
      <div className="adviceSection">
        <strong>Next 7 days</strong>
        <ul>
          {advice.body.next_7_days.map((item) => <li key={item}>{item}</li>)}
        </ul>
      </div>
      {(advice.body.questions ?? []).length > 0 && (
        <div className="adviceSection">
          <strong>Questions for you</strong>
          <ul>{advice.body.questions!.map((question) => <li key={question}>{question}</li>)}</ul>
        </div>
      )}
      {advice.body.data_sufficiency && (
        <div className="adviceSection">
          <strong>What this advice is based on</strong>
          <p>{advice.body.data_sufficiency}</p>
        </div>
      )}
      <div className="adviceSection recoverySection">
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
