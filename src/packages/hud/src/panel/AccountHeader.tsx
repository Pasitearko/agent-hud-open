import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { Check, Copy } from "lucide-react";
import type { Account } from "@agent-hud/protocol";
import { accountReadLabel, say } from "./account-labels";
import { useFormat } from "@agent-hud/ui/format-context";
import { MiddleTruncated } from "./MiddleTruncated";
import { noButtonChrome, palette } from "./theme";
import { useGround } from "../ground";
import { usePanelServices } from "../PanelContext";

/** The account above its windows (Open's AccountSectionHeader): its name,
 * shortened from the middle, its plan, and at the end of the line whether it
 * is current or when it was last read; under it the warning its client gave
 * (amber) and any other notice. Where the line is short the label gives way
 * first, then the name shortens. */
export function AccountHeader({
  account,
  now,
  light,
}: {
  account: Account;
  now: number;
  light: boolean;
}) {
  const colors = palette(useGround(light));
  const { language } = useFormat();
  const current = account.current !== false;
  const name = account.name ?? account.label ?? account.id;
  return (
    <div
      className="hud-source-account-heading"
      style={{
        display: "grid",
        gap: 2,
        padding: "2px 6px 0",
        fontSize: 11,
        lineHeight: "13px",
      }}
    >
      <div
        style={{ display: "flex", alignItems: "center", gap: 6, minHeight: 15 }}
      >
        <MiddleTruncated
          text={name}
          style={{
            flex: "0 1 auto",
            color: colors.text,
            opacity: current ? 0.85 : 0.6,
          }}
        />
        {account.planLabel && (
          <span
            style={{
              flex: "0 0 auto",
              color: colors.secondary,
              whiteSpace: "nowrap",
            }}
          >
            {account.planLabel}
          </span>
        )}
        <span style={{ flex: "1 1 8px", minWidth: 8 }} />
        <WholeOrNothing color={colors.tertiary}>
          {accountReadLabel(account, now, language)}
        </WholeOrNothing>
      </div>
      {account.warning && (
        <Notice text={account.warning} color={colors.warningText} />
      )}
      {account.notice && account.notice !== account.warning && (
        <Notice text={account.notice} color={colors.secondary} />
      )}
    </div>
  );
}

/** A block's notice where it has no account to stand under, as when its
 * client is signed out: set like the notice under an account's name. */
export function BlockNotice({ text, light }: { text: string; light: boolean }) {
  const colors = palette(useGround(light));
  return (
    <div
      className="hud-source-block-notice"
      style={{
        display: "grid",
        padding: "2px 6px 0",
        fontSize: 11,
        lineHeight: "13px",
      }}
    >
      <Notice text={text} color={colors.secondary} />
    </div>
  );
}

/** Shown whole or not at all; it gives way before anything else on its line. */
function WholeOrNothing({
  children,
  color,
}: {
  children: ReactNode;
  color: string;
}) {
  const box = useRef<HTMLSpanElement>(null);
  const [fits, setFits] = useState(true);
  useLayoutEffect(() => {
    const element = box.current;
    if (!element) return;
    const measure = () =>
      setFits(element.scrollWidth <= element.clientWidth + 0.5);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [children]);
  return (
    <span
      ref={box}
      style={{
        flex: "0 100 auto",
        minWidth: 0,
        overflow: "hidden",
        whiteSpace: "nowrap",
        color,
        visibility: fits ? undefined : "hidden",
      }}
    >
      {children}
    </span>
  );
}

/** A notice, cut to two lines unless `whole`. A click copies it whole: the
 * HUD never holds the keyboard, so ⌘C would copy from the app in front
 * instead. A copy mark fades in at its end on hover and turns into a check
 * once copied. */
export function Notice({
  text,
  color,
  whole = false,
}: {
  text: string;
  color: string;
  whole?: boolean;
}) {
  const copy = usePanelServices()?.copyText;
  const { language } = useFormat();
  const [hovered, setHovered] = useState(false);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(timer);
  }, [copied]);
  const words = (
    <span
      style={
        whole
          ? { color, display: "block" }
          : {
              color,
              display: "-webkit-box",
              WebkitLineClamp: 2,
              WebkitBoxOrient: "vertical",
              overflow: "hidden",
            }
      }
    >
      {text}
    </span>
  );
  if (!copy) return words;
  const Mark = copied ? Check : Copy;
  return (
    <button
      type="button"
      className="hud-notice"
      title={text}
      aria-label={
        copied
          ? say(language, "已复制", "Copied")
          : say(language, "复制", "Copy")
      }
      onPointerEnter={() => setHovered(true)}
      onPointerLeave={() => setHovered(false)}
      onClick={(event) => {
        event.stopPropagation();
        copy(text).then(
          () => setCopied(true),
          () => setCopied(false),
        );
      }}
      style={{
        ...noButtonChrome,
        position: "relative",
        display: "block",
        width: "100%",
        paddingRight: 16,
        cursor: "pointer",
        color,
      }}
    >
      {words}
      <span
        aria-hidden
        style={{
          position: "absolute",
          top: 1,
          right: 0,
          display: "inline-flex",
          opacity: hovered || copied ? 1 : 0,
          transition: "opacity 160ms ease-out",
        }}
      >
        <Mark size={11} strokeWidth={2.25} />
      </span>
    </button>
  );
}
