import { expect, test } from "@playwright/test";
import { workflowProgressPlan } from "../src/lib/workflowProgress";
import type { WorkflowDefinitionView, WorkflowRunView } from "../src/types/runtime";

const definition: WorkflowDefinitionView = {
  schemaVersion: 1, definitionVersion: 2, id: "robot", name: "Robot", description: "", rolePrompt: "",
  localSkillCount: 0, pluginSkillCount: 0, uniqueSkillCount: 0, skillCatalog: [],
  nodes: ["requirements", "design", "prototype"].map((id) => ({
    id, title: id, description: `${id} description`, localSkillCount: 0, pluginSkillCount: 0,
    skillDeclarationCount: 0, localSkillBindings: [], pluginSkillBindings: [],
  })),
};
const run: WorkflowRunView = {
  schemaVersion: 1, definitionVersion: 2, id: "run", threadId: "thread", workflowId: "robot",
  objective: "test", state: "active", currentNodeId: "prototype", currentNodeIndex: 2, nodeCount: 3,
  completedNodes: ["requirements", "design"].map((nodeId) => ({
    nodeId, summary: `${nodeId} complete`, evidence: ["document"], completedAtMs: 2,
  })),
  createdAtMs: 1, updatedAtMs: 3, revision: 3,
};

test("workflow projection preserves completion facts across cancellation and completion", () => {
  const project = (value: WorkflowRunView) => workflowProgressPlan("thread", value, [definition]);
  expect(project(run)?.steps.map((step) => step.status)).toEqual(["completed", "completed", "in_progress"]);
  expect(project({ ...run, state: "cancelled" })?.steps.map((step) => step.status))
    .toEqual(["completed", "completed", "pending"]);
  expect(project({ ...run, state: "completed", currentNodeId: null, currentNodeIndex: 3,
    completedNodes: [...run.completedNodes, { nodeId: "prototype", summary: "done", evidence: ["html"], completedAtMs: 4 }],
  })?.steps.map((step) => step.status)).toEqual(["completed", "completed", "completed"]);
  // A numeric position alone cannot assert that an unrecorded node was completed.
  expect(project({ ...run, completedNodes: [] })?.steps.map((step) => step.status))
    .toEqual(["pending", "pending", "in_progress"]);
});

test("workflow projection rejects cross-thread and incompatible definitions", () => {
  expect(workflowProgressPlan("other", run, [definition])).toBeNull();
  expect(workflowProgressPlan("thread", null, [definition])).toBeNull();
  expect(workflowProgressPlan("thread", run, [])).toBeNull();
  for (const change of [{ definitionVersion: 1 }, { nodeCount: 8 }, { currentNodeId: "removed" }]) {
    expect(workflowProgressPlan("thread", { ...run, ...change }, [definition])).toBeNull();
  }
});
