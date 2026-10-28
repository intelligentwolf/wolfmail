import * as React from "react";
import { CalendarDays, MapPin, Check, HelpCircle, X, AlertTriangle, Repeat, CalendarX2, MailCheck } from "lucide-react";
import { pimApi } from "../../lib/pimApi";
import { notify } from "../../lib/notify";
import { setMode, setPim } from "../../lib/pim";
import type { Invite } from "../../lib/pimTypes";
import { cn } from "../../lib/utils";
import { Button } from "../../components/ui/button";
import { Spinner } from "../../components/ui/misc";
import { allDayToLocal, fmtRange, fmtTime } from "./time";

const answerLabel: Record<string, string> = { ACCEPTED: "You're going", TENTATIVE: "You said maybe", DECLINED: "You declined", "NEEDS-ACTION": "No reply yet" };

/** Calendar invitation inside a message (iMIP): answer it, apply a cancellation, or record a guest's reply. */
export function InviteCard({ messageId, invite }: { messageId: string; invite: Invite }) {
  const [busy, setBusy] = React.useState<string | null>(null);
  const [mine, setMine] = React.useState(invite.myPartstat);
  const [applied, setApplied] = React.useState(false);

  const respond = async (r: "ACCEPTED" | "TENTATIVE" | "DECLINED") => {
    setBusy(r);
    try {
      await pimApi.inviteRespond(messageId, r);
      setMine(r);
      notify.ok(r === "ACCEPTED" ? "Accepted — it's in your calendar" : r === "TENTATIVE" ? "Replied maybe" : "Declined — the organiser has been told");
    } catch (e) {
      notify.error(e, "Couldn't send your reply");
    } finally {
      setBusy(null);
    }
  };
  const apply = async () => {
    setBusy("apply");
    try {
      await pimApi.inviteApply(messageId);
      setApplied(true);
      notify.ok(invite.method === "CANCEL" ? "Marked as cancelled in your calendar" : "Reply recorded on your event");
    } catch (e) {
      notify.error(e, "Couldn't update your calendar");
    } finally {
      setBusy(null);
    }
  };
  const showInCalendar = () => {
    setMode("calendar");
    setPim({ view: "week", anchor: invite.start });
  };

  const cancelled = invite.method === "CANCEL";
  const reply = invite.method === "REPLY";
  const tileDay = invite.allDay ? allDayToLocal(invite.start) : invite.start;
  return (
    <section aria-label="Calendar invitation" className={cn("mx-5 mt-3 overflow-hidden rounded-card ring-1", cancelled ? "ring-[var(--iw-red)]/40" : "ring-border")}>
      <div className={cn("flex items-start gap-3 px-4 py-3", cancelled ? "bg-[var(--iw-red-wash)]" : "bg-surface-raised")}>
        <div className="flex size-11 shrink-0 flex-col items-center justify-center rounded-xl bg-surface text-center ring-1 ring-border">
          <span className="text-[9.5px] font-semibold uppercase text-[var(--iw-red-text)]">{new Date(tileDay).toLocaleDateString(undefined, { month: "short" })}</span>
          <span className="text-[17px] font-semibold leading-none">{new Date(tileDay).getDate()}</span>
        </div>
        <div className="min-w-0 flex-1">
          <p className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
            {cancelled ? "Cancelled event" : reply ? "Reply to your invitation" : "Invitation"}
          </p>
          <h3 className={cn("truncate text-[15px] font-semibold", cancelled && "line-through")}>{invite.title || "(no title)"}</h3>
          <p className="flex items-center gap-1.5 text-[13px] text-muted-foreground"><CalendarDays className="size-3.5" />{fmtRange(invite.start, invite.end, invite.allDay)}{invite.rrule && <><Repeat className="ml-1 size-3.5" /> repeats</>}</p>
          {invite.location && <p className="flex items-center gap-1.5 truncate text-[13px] text-muted-foreground"><MapPin className="size-3.5" />{invite.location}</p>}
          {invite.organizer && !reply && <p className="text-xs text-muted-foreground">From {invite.organizer.name ?? invite.organizer.email} · {invite.attendees.length} {invite.attendees.length === 1 ? "guest" : "guests"}</p>}
          {reply && invite.replyFrom && <p className="text-[13px]"><b>{invite.replyFrom.name ?? invite.replyFrom.email}</b> — {answerLabel[invite.replyFrom.partstat] ?? invite.replyFrom.partstat}</p>}
        </div>
      </div>
      {invite.conflicts.length > 0 && !cancelled && !reply && (
        <div className="flex items-start gap-2 border-t border-border bg-surface px-4 py-2 text-[12.5px]">
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-[var(--iw-warn-text)]" />
          <span>Clashes with {invite.conflicts.map((c) => `${c.title} (${c.allDay ? "all day" : fmtTime(c.start)})`).join(", ")}</span>
        </div>
      )}
      <div className="flex flex-wrap items-center gap-2 border-t border-border bg-surface px-4 py-2.5">
        {cancelled || reply ? (
          invite.existingEventId ? (
            <Button size="sm" variant="outline" disabled={busy !== null || applied} onClick={apply}>
              {busy === "apply" ? <Spinner /> : cancelled ? <CalendarX2 /> : <MailCheck />}
              {applied ? "Done" : cancelled ? "Mark cancelled in my calendar" : "Update my event"}
            </Button>
          ) : (
            <span className="text-[12.5px] text-muted-foreground">This event isn't in your calendars.</span>
          )
        ) : invite.myEmail ? (
          <>
            <span className="mr-1 text-[12.5px] text-muted-foreground" aria-live="polite">{answerLabel[mine ?? "NEEDS-ACTION"] ?? "Going?"}</span>
            <Button size="sm" variant={mine === "ACCEPTED" ? "go" : "outline"} aria-pressed={mine === "ACCEPTED"} disabled={busy !== null} onClick={() => respond("ACCEPTED")}>{busy === "ACCEPTED" ? <Spinner /> : <Check />} Yes</Button>
            <Button size="sm" variant={mine === "TENTATIVE" ? "secondary" : "outline"} aria-pressed={mine === "TENTATIVE"} disabled={busy !== null} onClick={() => respond("TENTATIVE")}>{busy === "TENTATIVE" ? <Spinner /> : <HelpCircle />} Maybe</Button>
            <Button size="sm" variant="outline" aria-pressed={mine === "DECLINED"} disabled={busy !== null} onClick={() => respond("DECLINED")} className={cn(mine === "DECLINED" && "ring-[var(--iw-red)]")}>{busy === "DECLINED" ? <Spinner /> : <X />} No</Button>
          </>
        ) : (
          <span className="text-[12.5px] text-muted-foreground">None of your addresses is on the guest list.</span>
        )}
        <div className="flex-1" />
        <Button size="sm" variant="ghost" onClick={showInCalendar}><CalendarDays /> Show in calendar</Button>
      </div>
    </section>
  );
}
