import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Activity,
  CalendarDays,
  CheckCircle2,
  LogOut,
  RefreshCw,
  ShieldCheck,
  Sparkles,
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
import { api, ActivitySummary, StravaStatus, TrainingAdvice } from "./lib/api";

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
      <DashboardSummary activities={activities.data ?? []} />
      <TrendChart activities={activities.data ?? []} />

      <main className="layout">
        <ActivityTable
          activities={activities.data ?? []}
          isLoading={activities.isLoading}
          selectedId={selectedActivityId}
          onSelect={setSelectedActivityId}
        />
        <section className="panel">
          <h2>Activity Detail</h2>
          {selectedActivity.isLoading && <p className="muted">Loading activity...</p>}
          {!selectedActivityId && <p className="muted">Select an activity to inspect streams and raw Strava detail.</p>}
          {selectedActivity.data && (
            <dl className="details">
              <div><dt>Name</dt><dd>{selectedActivity.data.name}</dd></div>
              <div><dt>Distance</dt><dd>{metersToMiles(selectedActivity.data.distance_meters)} mi</dd></div>
              <div><dt>Moving time</dt><dd>{secondsToTime(selectedActivity.data.moving_time_seconds)}</dd></div>
              <div><dt>Heart rate</dt><dd>{selectedActivity.data.average_heartrate?.toFixed(0) ?? "n/a"} bpm</dd></div>
              <div><dt>Elevation</dt><dd>{metersToFeet(selectedActivity.data.total_elevation_gain)} ft</dd></div>
              <div><dt>Visibility</dt><dd>{selectedActivity.data.visibility ?? "n/a"}</dd></div>
            </dl>
          )}
        </section>
      </main>

      <AdvicePanel advice={advice.data ?? []} />
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

function AdvicePanel({ advice }: { advice: TrainingAdvice[] }) {
  const queryClient = useQueryClient();
  const generate = useMutation({
    mutationFn: () => api.generateAdvice(28),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["advice"] })
  });
  const latest = advice[0];

  return (
    <section className="panel">
      <div className="panelHeader">
        <h2>Training Advice</h2>
        <button onClick={() => generate.mutate()} disabled={generate.isPending}>
          <Sparkles size={16} /> Generate
        </button>
      </div>
      {!latest && <p className="muted">No advice generated yet.</p>}
      {latest && (
        <div className="advice">
          <strong>{latest.body.summary}</strong>
          <p>{latest.body.recovery_notes}</p>
          <ul>
            {latest.body.next_7_days.map((item) => <li key={item}>{item}</li>)}
          </ul>
          <small>{latest.body.safety_note}</small>
        </div>
      )}
    </section>
  );
}

function metersToMiles(value?: number | null) {
  return ((value ?? 0) / 1609.344).toFixed(1);
}

function metersToFeet(value?: number | null) {
  return ((value ?? 0) * 3.28084).toFixed(0);
}

function secondsToTime(value?: number | null) {
  const total = value ?? 0;
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  return hours > 0 ? `${hours}h ${minutes}m` : `${minutes}m`;
}
