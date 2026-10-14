import { describe, expect, it } from "vitest";
import { HIDDEN_BLOCK, MASK, maskWebhookKey, triggerBlock } from "@/components/workflows/triggers/triggerYaml";
import { TRIGGERS_SOURCE } from "./triggerFixtures";

describe("triggerBlock", () => {
  it("cuts one trigger's whole block from the source, dedented", () => {
    expect(triggerBlock(TRIGGERS_SOURCE, "hook")).toBe("id: hook\ntype: io.kestra.plugin.core.trigger.Webhook\nkey: s3cr3t-key");
    // A block with a nested list stops at the next trigger, not at its own list.
    expect(triggerBlock(TRIGGERS_SOURCE, "upstream")).toBe(
      "id: upstream\ntype: io.kestra.plugin.core.trigger.Flow\nconditions:\n  - type: io.kestra.plugin.core.condition.ExecutionStatus\n    in: [SUCCESS]",
    );
    // The last trigger runs to the end of the document.
    expect(triggerBlock(TRIGGERS_SOURCE, "poll")).toBe("id: poll\ntype: io.kestra.plugin.fs.http.Trigger\nuri: https://example.com");
  });

  it("keeps a less-indented comment inside the block, and stops at one that ends it", () => {
    const src = "triggers:\n  - id: a\n# about the cron\n    cron: x\n# the next trigger\n  - id: b\n    cron: y\n";
    expect(triggerBlock(src, "a")).toBe("id: a\n# about the cron\ncron: x");
    expect(triggerBlock(src, "b")).toBe("id: b\ncron: y");
  });

  it("is null for a flow-style list, which has no lines to cut", () => {
    expect(triggerBlock("id: x\ntriggers: [{ id: a, type: a.B }]\n", "a")).toBeNull();
  });

  it("is null for an id the source does not declare, or a task's id", () => {
    expect(triggerBlock(TRIGGERS_SOURCE, "gone")).toBeNull();
    expect(triggerBlock("id: x\ntasks:\n  - id: daily\n    type: a.B\n", "daily")).toBeNull();
  });
});

describe("maskWebhookKey", () => {
  const SECRET = "s3cr3t";
  /** Hidden: the secret is nowhere in what is shown, and only the key's value changed. */
  const expectKeyHidden = (block: string, expected: string) => {
    const out = maskWebhookKey(block);
    expect(out.text).not.toContain(SECRET);
    expect(out).toEqual({ text: expected, whole: false });
  };

  it("hides a plain, quoted or templated value", () => {
    expectKeyHidden("id: hook\ntype: a.Webhook\nkey: s3cr3t", `id: hook\ntype: a.Webhook\nkey: ${MASK}`);
    expectKeyHidden('id: hook\nkey: "s3cr3t" # the key', `id: hook\nkey: ${MASK} # the key`);
    expectKeyHidden("key: '{{ secret(\"s3cr3t\") }}'\nid: hook", `key: ${MASK}\nid: hook`);
  });

  it("(a) hides it in a flow-mapping trigger", () => {
    expectKeyHidden("{ id: hook, type: a.Webhook, key: s3cr3t }", `{ id: hook, type: a.Webhook, key: ${MASK} }`);
  });

  it("(b) hides it under a quoted key name", () => {
    expectKeyHidden('id: hook\n"key": s3cr3t', `id: hook\n"key": ${MASK}`);
    expectKeyHidden("id: hook\n'key': s3cr3t", `id: hook\n'key': ${MASK}`);
  });

  it("(c) hides a value that runs over several lines", () => {
    expectKeyHidden('key: "abc\n  s3cr3t"\nid: hook', `key: ${MASK}\nid: hook`);
    expectKeyHidden("key: abc\n  s3cr3t\nid: hook", `key: ${MASK}\nid: hook`);
    expectKeyHidden("key: |\n  s3cr3t\n  more\nid: hook", `key: ${MASK}\nid: hook`);
  });

  it("(d) hides a sequence at the key's own indent", () => {
    expectKeyHidden("key:\n- s3cr3t\nid: hook", `key: ${MASK}\nid: hook`);
  });

  it("hides the whole block when it does not parse, or the key cannot be told apart", () => {
    for (const block of [
      "id: hook\nkey: s3cr3t\nbad: : :", // does not parse
      "- id: hook\n  key: s3cr3t", // not a mapping
      "id: hook\nkey: s3cr3t\nkey: s3cr3t", // two keys
      "id: hook\nother: &k s3cr3t\nkey: *k", // an alias to text elsewhere
      "id: hook\n? [key]\n: s3cr3t", // a complex key
      "id: hook\nhookKey: s3cr3t", // no key at all
      // The value starts after a comment on the key's line: the mask would land inside the comment
      // and leave `key` empty, which the re-parse sees.
      "key: # the key\n  s3cr3t\nid: hook",
    ]) {
      expect(maskWebhookKey(block)).toEqual({ text: HIDDEN_BLOCK, whole: true });
    }
  });

  it("leaves a nested key as it is, hiding only the trigger's own", () => {
    expectKeyHidden("id: hook\nkey: s3cr3t\nconditions:\n  - key: other", `id: hook\nkey: ${MASK}\nconditions:\n  - key: other`);
  });
});
