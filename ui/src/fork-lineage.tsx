export default function ForkLineage({
  parentSessionId,
  parentTurnId,
}: {
  parentSessionId?: string | null;
  parentTurnId?: string | null;
}) {
  if (!parentSessionId) return null;
  return (
    <span className="tag" title={`Forked at turn ${parentTurnId ?? "unknown"}`}>
      fork of {parentSessionId.slice(0, 8)}
    </span>
  );
}
