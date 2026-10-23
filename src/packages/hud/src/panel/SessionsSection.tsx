import type {
  PanelLayout,
  SessionsResponse,
  Snapshot,
} from "@agent-hud/protocol";
import { formatTokens } from "../quota-metrics";
import { sourceID, sourcesForMark } from "../model";
import { usePanelQuery, usePanelServices } from "../PanelContext";
import { noButtonChrome, palette, tabular, type Logo } from "./theme";
import { useGround } from "../ground";
import { useFormat } from "@agent-hud/ui/format-context";
import { useListedSessions } from "./listed-sessions";
import { noSessionsMatch } from "./session-filters";

export function SourceSessions({
  snapshot,
  focusVendor,
  light,
  Logo,
  onStats,
  layout = {},
  active = true,
}: {
  snapshot: Snapshot;
  focusVendor?: string;
  light: boolean;
  Logo: Logo;
  onStats?: () => void;
  layout?: PanelLayout["sessions"];
  active?: boolean;
}) {
  const zh = useFormat().language === "zh";
  const services = usePanelServices();
  const listed = useListedSessions();
  // The Sessions page: the list, or one session's usage.
  const openSessions = (sessionId?: string) =>
    listed ? listed.openStats({ tab: "sessions", sessionId }) : onStats?.();
  const limit = layout.limit ?? 5;
  const selectedSources = focusVendor
    ? sourcesForMark(snapshot, focusVendor)
    : layout.agents?.length
      ? layout.agents.map((source) => sourceID(snapshot, source))
      : undefined;
  const idleSources = Object.entries(services?.sources ?? {})
    .filter(([, prefs]) => prefs.live === false)
    .map(([source]) => source);
  const query = usePanelQuery<SessionsResponse>(
    "sessions",
    {
      scope: layout.period === "week" ? "recent" : "active",
      limit,
      sort: layout.sort ?? "active",
      ...(selectedSources ? { sources: selectedSources } : {}),
      ...(layout.projects?.length ? { projects: layout.projects } : {}),
      ...(layout.states?.length ? { states: layout.states } : {}),
      ...(idleSources.length ? { idleSources } : {}),
    },
    snapshot.sessionsRevision,
    active,
  );
  const colors = palette(useGround(light));
  // Snapshot rows provide a first paint only; the complete-index query owns
  // ordering and totals, including running sessions beyond the first page.
  const fallback = snapshot.sessions
    .map((session) =>
      idleSources.includes(sourceID(snapshot, session.source))
        ? { ...session, state: "idle" as const }
        : session,
    )
    .filter(
      (session) =>
        (!selectedSources ||
          selectedSources.includes(sourceID(snapshot, session.source))) &&
        (!layout.projects?.length ||
          layout.projects.includes(
            session.project ?? session.cwd ?? "unassigned",
          )) &&
        (!layout.states?.length || layout.states.includes(session.state)),
    )
    .sort((a, b) => {
      const rank = (state: string) =>
        state === "waiting" ? 0 : state === "running" ? 1 : 2;
      return (
        (layout.sort === "date" ? 0 : rank(a.state) - rank(b.state)) ||
        Date.parse(b.updatedAt) - Date.parse(a.updatedAt)
      );
    });
  const sessions = query.data?.items ?? fallback.slice(0, limit);
  const total = query.data?.total ?? fallback.length;
  const activeCount =
    query.data?.activeCount ??
    fallback.filter(
      (session) => session.state === "running" || session.state === "waiting",
    ).length;
  // A filter of the layout that keeps none of the sessions there are says
  // so, and the header then claims no state of sessions it leaves out.
  const own = focusVendor
    ? snapshot.sessions.filter((session) =>
        selectedSources!.includes(sourceID(snapshot, session.source)),
      ).length
    : snapshot.sessions.length;
  const noneMatch = noSessionsMatch(
    layout,
    Boolean(focusVendor),
    sessions.length,
    own,
  );
  const status = activeCount
    ? zh
      ? `${activeCount} 个运行中`
      : `${activeCount} running`
    : sessions.some((session) => session.state === "unverified")
      ? zh
        ? "状态未知"
        : "Status unavailable"
      : zh
        ? "最近结束"
        : "Recently ended";
  return (
    <section
      className="hud-source-sessions"
      style={{
        display: "grid",
        gap: "var(--hud-session-gap, 6px)",
        paddingTop: "var(--hud-divided-gap, 8px)",
        boxShadow: `inset 0 1px 0 ${colors.divider}`,
        fontSize: 12,
      }}
    >
      <button
        title={zh ? "查看会话列表" : "Show sessions"}
        onClick={() => openSessions()}
        disabled={!onStats && !listed}
        style={{
          ...noButtonChrome,
          display: "flex",
          alignItems: "baseline",
          gap: 8,
          width: "100%",
          fontSize: 13,
          lineHeight: "16px",
        }}
      >
        <strong style={{ fontWeight: 600, color: colors.text }}>
          {zh ? "会话" : "Sessions"}
        </strong>
        <span
          style={{
            marginLeft: "auto",
            ...tabular,
            fontSize: 12,
            color: colors.secondary,
            // Without a status the line keeps its room.
            visibility: noneMatch ? "hidden" : undefined,
          }}
        >
          {status}
        </span>
      </button>
      {sessions.slice(0, limit).map((session) => (
        // The whole row opens the session's page, as its heading opens the list.
        <button
          key={session.id}
          disabled={!onStats && !listed}
          title={`${session.title}\n${
            zh ? "查看会话详情" : "Show session details"
          }`}
          onClick={() =>
            listed
              ? openSessions(session.id)
              : services?.selectSession
                ? services.selectSession(session)
                : onStats?.()
          }
          style={{
            ...noButtonChrome,
            display: "flex",
            alignItems: "center",
            gap: 8,
            width: "100%",
            color: colors.secondary,
            lineHeight: "15px",
            minHeight: 15,
          }}
        >
          <span
            style={{
              position: "relative",
              width: 14,
              height: 14,
              flex: "0 0 14px",
              opacity: session.state === "idle" ? 0.7 : 1,
            }}
          >
            {/* The ring round the dot is cut out of the mark, so the ground shows in it. */}
            <span
              style={{
                display: "block",
                width: 14,
                height: 14,
                WebkitMaskImage:
                  "radial-gradient(circle at 13px 13px, transparent 4.5px, #000 5px)",
                maskImage:
                  "radial-gradient(circle at 13px 13px, transparent 4.5px, #000 5px)",
              }}
            >
              <Logo source={session.source} size={14} light={light} />
            </span>
            <i
              style={{
                position: "absolute",
                right: -2,
                bottom: -2,
                width: 6,
                height: 6,
                borderRadius: "50%",
                background:
                  session.state === "waiting"
                    ? colors.warning
                    : session.state === "running"
                      ? "#c084fc"
                      : colors.tertiary,
              }}
            />
          </span>
          <span
            style={{
              flex: 1,
              minWidth: 0,
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
            }}
          >
            {sessionTitle(session.title, session.cwd) ||
              (zh ? "（未命名会话）" : "(untitled session)")}
          </span>
          <span style={{ ...tabular, fontSize: 12, whiteSpace: "nowrap" }}>
            {formatTokens(session.tokens)} tok
          </span>
        </button>
      ))}
      {!sessions.length && (
        <span style={{ color: colors.secondary, lineHeight: "15px" }}>
          {noneMatch
            ? zh
              ? "没有符合条件的会话"
              : "No sessions match"
            : zh
              ? "还没有会话"
              : "No sessions yet"}
        </span>
      )}
      {total > limit && (
        <button
          style={{ ...noButtonChrome, color: colors.secondary, fontSize: 12 }}
          disabled={!onStats && !listed}
          onClick={() => openSessions()}
        >
          {zh ? `还有 ${total - limit} 个` : `+${total - limit} more`}
        </button>
      )}
    </section>
  );
}

/** The row's title (Open's HoverPanelView.sessionTitle): the task cut short,
 * then the session's folder, so rows of one task in different folders differ. */
export function sessionTitle(task: string, cwd: string | null) {
  const terminal = cwd?.split(/[\\/]/).filter(Boolean).pop();
  if (!terminal || !task) return task;
  const words = task.split(" ").filter(Boolean);
  const short =
    words.length > 3
      ? words.slice(0, 3).join(" ")
      : [...task].slice(0, 14).join("");
  return `${short} · ${terminal}`;
}
