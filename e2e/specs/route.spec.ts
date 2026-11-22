import { browser, $, expect } from "@wdio/globals";
import {
  approvalCard,
  approveEveryCard,
  openRail,
  openSettingsPage,
  openUtility,
  sendPrompt,
  startSession,
  textCount,
  waitForText,
} from "../lib/app";

/**
 * Roadmap 13: the route inspector and `turn_send` must report the same RouteExplanation.
 *
 * The turn stores what `turn_send` returned; `route_explain` is the command the inspector reads.
 * Feeding the turn's own request back through `route_explain` and comparing field for field is the
 * equality the roadmap asks for. This spec drives two real turns with no requested provider: the
 * first is `model-only` because the saved door accepts the spawned row's id, and the second is a
 * `bare-id-repair` after the roster picker writes an id that another door owns. Both are replayed
 * with the exact input each turn carried, so the repair case does not weaken the equality check.
 *
 * The runner seeds this spec with Groq / `llama-4`: Groq accepts the saved id but refuses a bare
 * DeepSeek id, and `deepseek-chat` unambiguously names DeepSeek's own door. No other spec sees that
 * fixture. The inspector's rendered rows are also checked against the resolver, so the panel
 * cannot drift from it either.
 */
type Route = { provider: string; model: string };
type Explanation = {
  saved: Route;
  requested: Route | null;
  resolved: Route;
  reason: string;
  mismatch: string | null;
};
type E2eConfig = { project: string };
type Session = { id: string; status: string };
type Agent = {
  id: string;
  harness: string;
  model: string | null;
  provider?: string | null;
};

const BARE_MODEL = "deepseek-chat";

/** Invoke one production command through the same Tauri bridge the window uses. */
async function invoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  return (await browser.execute(
    (name: string, payload: Record<string, unknown>) => {
      const bridge = (
        window as unknown as {
          __TAURI_INTERNALS__?: {
            invoke: (command: string, args: Record<string, unknown>) => Promise<unknown>;
          };
        }
      ).__TAURI_INTERNALS__;
      if (!bridge) throw new Error("the Tauri IPC bridge is not available");
      return bridge.invoke(name, payload);
    },
    command,
    args,
  )) as T;
}

async function lastTurnRoute(): Promise<Explanation> {
  const route = (await browser.execute(
    () =>
      (window as unknown as { __rigelLastTurnRoute?: Explanation }).__rigelLastTurnRoute ?? null,
  )) as Explanation | null;
  expect(route).not.toBeNull();
  return route as Explanation;
}

/** The exact input the turn carried, replayed through the command the inspector reads. */
async function expectExactReplay(
  route: Explanation,
  provider: string | null,
  model: string | null,
) {
  const explain = await invoke<Explanation>("route_explain", { provider, model });
  expect(explain).toEqual(route);
}

async function currentProject(): Promise<string> {
  const config = await invoke<E2eConfig | null>("e2e_config");
  expect(config?.project).toBeTruthy();
  return (config as E2eConfig).project;
}

async function currentSession(project: string): Promise<string> {
  const sessions = await invoke<Session[]>("session_list", { project });
  const session = sessions.find((row) => row.status !== "ended") ?? sessions[0];
  expect(session).toBeDefined();
  return (session as Session).id;
}

async function currentAgent(project: string, session: string): Promise<Agent> {
  const agents = await invoke<Agent[]>("agent_list", { project, session });
  const agent = agents.find((row) => row.harness === "native");
  expect(agent).toBeDefined();
  return agent as Agent;
}

describe("the route inspector", () => {
  it("reports the same explanation the turn used, including a bare-id repair", async () => {
    await startSession();
    // Wait for the saved pair, not the composer's initial fallback: the spawn copies this model
    // into the agent row, and that row has no provider, which is what makes the first turn
    // model-only rather than a repair of the fallback id.
    await waitForText("llama-4");
    await sendPrompt("Route check.");
    const firstCard = await approvalCard();
    await firstCard.waitForDisplayed({ timeout: 30_000 });
    await approveEveryCard();
    await waitForText("Done.");

    const modelOnly = await lastTurnRoute();
    expect(modelOnly.reason).toBe("model-only");
    expect(modelOnly.requested).toBeNull();
    // The spawned row carried the saved model and no provider. ModelOnly means the saved door
    // accepted that exact id, so replaying the resolved id is the same bare input, not a
    // reconstruction from the answer.
    await expectExactReplay(modelOnly, null, modelOnly.resolved.model);

    // The roster picker's own command writes a bare id to the row. Returning to Session remounts
    // the surface, so the composer reads the row back; the next Send is a real turn through
    // deliver -> turn_send, with the same approvals and scripted loop as the first.
    const project = await currentProject();
    const session = await currentSession(project);
    const agent = await currentAgent(project, session);
    const configured = await invoke<Agent>("agent_configure", {
      project,
      session,
      agent: agent.id,
      model: BARE_MODEL,
      provider: null,
      effort: "medium",
    });
    expect(configured.model).toBe(BARE_MODEL);
    expect(configured.provider).toBeNull();

    await openSettingsPage("Agent");
    await openRail("Session");
    await waitForText("deepseek-chat");
    await waitForText("Route check.");
    await sendPrompt("Bare repair check.");

    const repairCard = await approvalCard();
    await repairCard.waitForDisplayed({ timeout: 30_000 });
    await approveEveryCard(4, 2);
    await browser.waitUntil(async () => (await textCount("Done.")) >= 2, {
      timeout: 60_000,
      interval: 200,
      timeoutMsg: "the bare-model turn did not finish",
    });

    const repaired = await lastTurnRoute();
    expect(repaired.reason).toBe("bare-id-repair");
    expect(repaired.requested).toBeNull();
    expect(repaired.saved.provider).toBe("groq");
    expect(repaired.resolved).toEqual({ provider: "deepseek", model: BARE_MODEL });
    // The exact input was the bare id with no provider. Replaying it must return the whole
    // explanation unchanged; the equality is still field for field.
    await expectExactReplay(repaired, null, BARE_MODEL);
  });

  it("shows selected, requested, resolved, reason and the mismatch sentence", async () => {
    await openUtility("Settings");
    const inspector = await $('section[aria-label="Route inspector"]');
    await inspector.waitForDisplayed({ timeout: 20_000 });

    const saved = await (await inspector.$('[data-testid="route-selected"]')).getText();
    expect(saved).toContain("/");
    const [savedProvider] = saved.split(" / ");

    await (await inspector.$('input[aria-label="Requested provider"]')).setValue(savedProvider);
    await (await inspector.$('input[aria-label="Requested model"]')).setValue("e2e/probe-model");

    await browser.waitUntil(
      async () =>
        (await (await inspector.$('[data-testid="route-reason"]')).getText()) === "picker",
      { timeout: 10_000, timeoutMsg: "the inspector did not resolve the previewed pair" },
    );
    expect(await (await inspector.$('[data-testid="route-resolved"]')).getText()).toBe(
      `${savedProvider} / e2e/probe-model`,
    );
    expect(await (await inspector.$('[data-testid="route-requested"]')).getText()).toContain(
      savedProvider,
    );
    expect(await (await inspector.$('[data-testid="route-mismatch"]')).getText()).toContain(
      "asked for",
    );
  });
});
