package cmd

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/ldclabs/anda-brain/anda-cli/api"
	"github.com/spf13/cobra"
)

// memorySession is the CLI's session file (MI §5.1, the MemorySession
// helper): the receipts a later recall must wait on and the last attention
// cursor the caller consumed. A receipt leaves only when a recall accounted
// for it, as available or as terminally failed; a maximum sequence never
// replaces one.
type memorySession struct {
	SpaceID         string           `json:"space_id"`
	Scope           *api.MemoryScope `json:"scope,omitempty"`
	Outstanding     []string         `json:"outstanding"`
	AttentionCursor string           `json:"attention_cursor,omitempty"`
}

func loadMemorySession(path, space string) (*memorySession, error) {
	if path == "" {
		return nil, nil
	}
	data, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return &memorySession{SpaceID: space, Outstanding: []string{}}, nil
	}
	if err != nil {
		return nil, fmt.Errorf("read session %q: %w", path, err)
	}
	var session memorySession
	if err := json.Unmarshal(data, &session); err != nil {
		return nil, fmt.Errorf("session %q: %w", path, err)
	}
	if session.SpaceID != space {
		return nil, fmt.Errorf("session %q belongs to space %q, not %q", path, session.SpaceID, space)
	}
	if space == "" || utf8.RuneCountInString(space) > 512 || len(session.Outstanding) > api.MaxAfter {
		return nil, fmt.Errorf("invalid space or receipt count in session %q", path)
	}
	session.Outstanding, err = mergeMemoryRefs("outstanding receipts", session.Outstanding)
	if err != nil {
		return nil, fmt.Errorf("session %q: %w", path, err)
	}
	if session.AttentionCursor != "" && !validMemoryRef(session.AttentionCursor) {
		return nil, fmt.Errorf("invalid attention cursor in session %q", path)
	}
	if scope := session.Scope; scope != nil {
		if scope.TaskRef != "" && !validMemoryRef(scope.TaskRef) {
			return nil, fmt.Errorf("invalid task reference in session %q", path)
		}
		// A saved scope's order participates in the CLI's existing retry key.
		scope.ContextRefs, err = mergeMemoryRefs("context references", scope.ContextRefs)
		if err != nil {
			return nil, fmt.Errorf("session %q: %w", path, err)
		}
	}
	return &session, nil
}

func validMemoryRef(ref string) bool {
	return ref != "" && utf8.RuneCountInString(ref) <= 1024
}

// Keep the first occurrence so repeated handles occupy one slot and retain their order.
func mergeMemoryRefs(name string, groups ...[]string) ([]string, error) {
	merged := []string{}
	for _, refs := range groups {
		for _, ref := range refs {
			if !validMemoryRef(ref) {
				return nil, fmt.Errorf("%s must contain references of 1 to 1024 characters", name)
			}
			if slices.Contains(merged, ref) {
				continue
			}
			if len(merged) == api.MaxAfter {
				return nil, fmt.Errorf("at most %d distinct %s", api.MaxAfter, name)
			}
			merged = append(merged, ref)
		}
	}
	return merged, nil
}

func (s *memorySession) save(path string) error {
	if s == nil || path == "" {
		return nil
	}
	data, err := json.MarshalIndent(s, "", "  ")
	if err != nil {
		return err
	}
	if dir := filepath.Dir(path); dir != "" {
		if err := os.MkdirAll(dir, 0o700); err != nil {
			return err
		}
	}
	temp := path + ".tmp"
	if err := os.WriteFile(temp, data, 0o600); err != nil {
		return err
	}
	return os.Rename(temp, path)
}

// recordReceipt keeps a receipt as a barrier until a recall accounts for it.
func (s *memorySession) recordReceipt(receipt *api.MemoryReceipt) error {
	if s == nil || receipt == nil {
		return nil
	}
	if receipt.SpaceID != s.SpaceID || !validMemoryRef(receipt.ReceiptRef) {
		return fmt.Errorf("receipt has an invalid reference or belongs to a different space")
	}
	if slices.Contains(s.Outstanding, receipt.ReceiptRef) {
		return nil
	}
	if len(s.Outstanding) >= api.MaxAfter {
		return fmt.Errorf("the session already holds %d outstanding receipts; recall with them before submitting more", api.MaxAfter)
	}
	s.Outstanding = append(s.Outstanding, receipt.ReceiptRef)
	return nil
}

// acknowledgeRecall clears the receipts a recall accounted for: available and
// not still pending, or failed. A failed receipt is terminal and never becomes
// available, so keeping it would leave every later recall pending; the recall
// that reported it already carries its failure in the printed briefing.
func (s *memorySession) acknowledgeRecall(briefing *api.MemoryBriefing) {
	if s == nil || briefing == nil {
		return
	}
	accounted := map[string]bool{}
	for _, progress := range briefing.After {
		switch progress.Phase {
		case "failed":
			accounted[progress.ReceiptRef] = true
		case "available":
			if !slices.Contains(briefing.Coverage.PendingReceipts, progress.ReceiptRef) {
				accounted[progress.ReceiptRef] = true
			}
		}
	}
	s.Outstanding = slices.DeleteFunc(s.Outstanding, func(receipt string) bool { return accounted[receipt] })
	if briefing.AttentionCursor != "" {
		s.AttentionCursor = briefing.AttentionCursor
	}
}

// memoryKey is a retry-safe idempotency key derived from the logical input,
// so re-running the same command replays instead of submitting again.
func memoryKey(operation string, scope *api.MemoryScope, input any) string {
	data, _ := json.Marshal([]any{operation, scope, input})
	sum := sha256.Sum256(data)
	return "cli:" + operation + ":" + hex.EncodeToString(sum[:12])
}

func memoryScopeFlags(cmd *cobra.Command, session *memorySession) *api.MemoryScope {
	task, _ := cmd.Flags().GetString("task")
	contexts, _ := cmd.Flags().GetStringArray("context")
	if task == "" && len(contexts) == 0 {
		if session != nil {
			return session.Scope
		}
		return nil
	}
	return &api.MemoryScope{TaskRef: task, ContextRefs: contexts}
}

func memoryBudgetFlags(cmd *cobra.Command) *api.MemoryBudget {
	var budget api.MemoryBudget
	set := false
	if cmd.Flags().Changed("max-tokens") {
		value, _ := cmd.Flags().GetInt64("max-tokens")
		budget.MaxOutputTokens = &value
		set = true
	}
	if cmd.Flags().Changed("deadline-ms") {
		value, _ := cmd.Flags().GetInt64("deadline-ms")
		budget.DeadlineMs = &value
		set = true
	}
	if !set {
		return nil
	}
	return &budget
}

// sendMemory sends one intent, keeps its receipt in the session and prints
// the Response. A failed intent is printed and reported as an error.
func sendMemory(cmd *cobra.Command, operation string, input map[string]any, keyed bool) error {
	sessionPath, _ := cmd.Flags().GetString("session")
	session, err := loadMemorySession(sessionPath, spaceID)
	if err != nil {
		return err
	}
	scope := memoryScopeFlags(cmd, session)
	if operation == "recall" {
		requested, _ := cmd.Flags().GetStringArray("after")
		var outstanding []string
		if session != nil {
			outstanding = session.Outstanding
			mode, _ := input["mode"].(string)
			if input["attention_cursor"] == nil && session.AttentionCursor != "" && (mode == "attention" || mode == "resume") {
				input["attention_cursor"] = session.AttentionCursor
			}
		}
		after, err := mergeMemoryRefs("after receipts", requested, outstanding)
		if err != nil {
			return err
		}
		if len(after) > 0 {
			input["after"] = after
		}
	} else if session != nil && len(session.Outstanding) >= api.MaxAfter {
		return fmt.Errorf("recall the session's %d outstanding receipts before submitting another mutation", api.MaxAfter)
	}
	raw, err := json.Marshal(input)
	if err != nil {
		return err
	}
	request := &api.MemoryRequest{
		KipMemory: api.MemoryInterfaceVersion,
		Operation: operation,
		Scope:     scope,
		Budget:    memoryBudgetFlags(cmd),
		Input:     raw,
	}
	if keyed {
		key, _ := cmd.Flags().GetString("key")
		if key == "" {
			key = memoryKey(operation, scope, input)
		}
		request.IdempotencyKey = key
	}
	response, err := newClient().Memory(cmd.Context(), request)
	if err != nil {
		return err
	}
	if response.Receipt != nil && response.Status != "failed" {
		err = session.recordReceipt(response.Receipt)
	}
	if operation == "recall" && response.Status != "failed" && len(response.Result) > 0 {
		var briefing api.MemoryBriefing
		if err := json.Unmarshal(response.Result, &briefing); err == nil {
			session.acknowledgeRecall(&briefing)
		}
	}
	if session != nil && response.Status != "failed" {
		session.Scope = scope
	}
	if err == nil {
		err = session.save(sessionPath)
	}
	if printErr := printJSON(cmd, response); printErr != nil {
		return printErr
	}
	if err != nil {
		return fmt.Errorf("updating memory session: %w", err)
	}
	if response.Error != nil {
		return response.Error
	}
	if response.Status == "failed" {
		return fmt.Errorf("memory %s failed", operation)
	}
	return nil
}

var memorySourcesCmd = &cobra.Command{Use: "sources", Short: "Stage captured sources for the Memory Interface"}

var memorySourcesStageCmd = &cobra.Command{
	Use:   "stage",
	Short: "Stage observed messages (--text, --file or stdin) and print their source_ref",
	Args:  cobra.NoArgs,
	RunE: func(cmd *cobra.Command, args []string) error {
		stream, _ := cmd.Flags().GetString("stream")
		event, _ := cmd.Flags().GetString("event")
		if stream == "" && (cmd.Flags().Changed("event") || cmd.Flags().Changed("ordinal") || cmd.Flags().Changed("after")) {
			return fmt.Errorf("--event, --ordinal and --after require --stream")
		}
		if stream != "" && event == "" {
			return fmt.Errorf("--event is required with --stream")
		}
		raw, err := readInput(cmd, "text", "file")
		if err != nil {
			return err
		}
		messages, err := parseMessagesInput(raw)
		if err != nil {
			return err
		}
		input := &api.StageSourceInput{Messages: messages}
		input.ObservedAt, _ = cmd.Flags().GetString("observed-at")
		input.Kind, _ = cmd.Flags().GetString("kind")
		if stream != "" {
			ordinal, _ := cmd.Flags().GetUint64("ordinal")
			after, _ := cmd.Flags().GetStringArray("after")
			input.Order = &api.SourceOrder{StreamRef: stream, EventRef: event, Ordinal: ordinal, PredecessorReceipts: after}
		}
		input.IdempotencyKey, _ = cmd.Flags().GetString("key")
		if input.IdempotencyKey == "" {
			input.IdempotencyKey = memoryKey("source", nil, input)
		}
		response, err := newClient().StageMemorySource(cmd.Context(), input)
		if err != nil {
			return err
		}
		return printRPC(cmd, response)
	},
}

var memorySourcesGetCmd = &cobra.Command{
	Use:   "get <source_ref>",
	Short: "Read a staged source",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		response, err := newClient().MemorySource(cmd.Context(), args[0])
		if err != nil {
			return err
		}
		return printRPC(cmd, response)
	},
}

var memoryObserveCmd = &cobra.Command{
	Use:   "observe --source <source_ref>",
	Short: "Memory Interface observe: form memory from a staged source",
	Args:  cobra.NoArgs,
	RunE: func(cmd *cobra.Command, args []string) error {
		source, _ := cmd.Flags().GetString("source")
		if source == "" {
			return fmt.Errorf("--source is required")
		}
		return sendMemory(cmd, "observe", map[string]any{"source_ref": source}, true)
	},
}

var memoryReviseCmd = &cobra.Command{
	Use:   "revise --source <source_ref> --kind <kind>",
	Short: "Memory Interface revise: correction, world_change, misrecorded or unspecified",
	Args:  cobra.NoArgs,
	RunE: func(cmd *cobra.Command, args []string) error {
		source, _ := cmd.Flags().GetString("source")
		if source == "" {
			return fmt.Errorf("--source is required")
		}
		input := map[string]any{"source_ref": source}
		if kind, _ := cmd.Flags().GetString("kind"); kind != "" {
			switch kind {
			case "correction", "world_change", "misrecorded", "unspecified":
				input["change_kind"] = kind
			default:
				return fmt.Errorf("--kind is correction, world_change, misrecorded or unspecified")
			}
		}
		if target, _ := cmd.Flags().GetString("target"); target != "" {
			input["target_ref"] = target
		}
		return sendMemory(cmd, "revise", input, true)
	},
}

var memoryFeedbackCmd = &cobra.Command{
	Use:   "feedback --source <source_ref>",
	Short: "Memory Interface feedback: preserve a report with its origin (never a grade)",
	Args:  cobra.NoArgs,
	RunE: func(cmd *cobra.Command, args []string) error {
		source, _ := cmd.Flags().GetString("source")
		if source == "" {
			return fmt.Errorf("--source is required")
		}
		input := map[string]any{"source_ref": source}
		if decision, _ := cmd.Flags().GetString("decision"); decision != "" {
			input["decision_ref"] = decision
		}
		if attempt, _ := cmd.Flags().GetString("attempt"); attempt != "" {
			input["attempt_ref"] = attempt
		}
		return sendMemory(cmd, "feedback", input, true)
	},
}

var memoryRecallCmd = &cobra.Command{
	Use:   "recall [query]",
	Short: "Memory Interface recall: answer, action, resume or attention",
	Args:  cobra.MaximumNArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		input := map[string]any{}
		if len(args) == 1 && strings.TrimSpace(args[0]) != "" {
			input["query"] = args[0]
		}
		mode, _ := cmd.Flags().GetString("mode")
		if mode != "" {
			input["mode"] = mode
		}
		if target, _ := cmd.Flags().GetString("target"); target != "" {
			input["target_ref"] = target
		}
		if detail, _ := cmd.Flags().GetString("detail"); detail != "" {
			input["detail"] = detail
		}
		if context, _ := cmd.Flags().GetString("situation"); context != "" {
			input["context"] = context
		}
		cursor, _ := cmd.Flags().GetString("attention-cursor")
		if cursor != "" {
			input["attention_cursor"] = cursor
		}
		time := map[string]any{}
		if validAt, _ := cmd.Flags().GetString("valid-at"); validAt != "" {
			time["valid_at"] = validAt
		}
		if cmd.Flags().Changed("as-of-seq") {
			seq, _ := cmd.Flags().GetUint64("as-of-seq")
			time["as_of_seq"] = seq
		}
		if len(time) > 0 {
			input["time"] = time
		}
		return sendMemory(cmd, "recall", input, false)
	},
}

var memoryReceiptCmd = &cobra.Command{
	Use:   "receipt <receipt_ref>",
	Short: "Read a Memory Interface receipt's current progress",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		response, err := newClient().MemoryReceiptView(cmd.Context(), args[0])
		if err != nil {
			return err
		}
		return printRPC(cmd, response)
	},
}

var memoryPlanCmd = &cobra.Command{
	Use:   "plan <plan_ref>",
	Short: "Read a forget's ErasurePlan",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		response, err := newClient().MemoryPlan(cmd.Context(), args[0])
		if err != nil {
			return err
		}
		return printRPC(cmd, response)
	},
}

func addMemoryIntentFlags(cmd *cobra.Command, keyed bool) {
	cmd.Flags().String("task", "", "Scope: an authorized task handle")
	cmd.Flags().StringArray("context", nil, "Scope: a context handle (repeatable)")
	cmd.Flags().Int64("max-tokens", 0, "Output budget in tokens")
	cmd.Flags().Int64("deadline-ms", 0, "Deadline in milliseconds (a mutation waits for its processing up to this)")
	cmd.Flags().String("session", "", "Session file keeping outstanding receipts and the attention cursor")
	if keyed {
		cmd.Flags().String("key", "", "Idempotency key (default: derived from the operation, scope and input)")
	}
}

func init() {
	memorySourcesStageCmd.Flags().String("file", "", "Messages JSON file (array, object, or plain text)")
	memorySourcesStageCmd.Flags().String("text", "", "One user message as plain text")
	memorySourcesStageCmd.Flags().String("observed-at", "", "When the source was observed (RFC 3339)")
	memorySourcesStageCmd.Flags().String("kind", "", "message, tool_trace or artifact")
	memorySourcesStageCmd.Flags().String("stream", "", "Source order: stream ref")
	memorySourcesStageCmd.Flags().String("event", "", "Source order: event ref")
	memorySourcesStageCmd.Flags().Uint64("ordinal", 0, "Source order: ordinal in the stream")
	memorySourcesStageCmd.Flags().StringArray("after", nil, "Source order: predecessor receipt (repeatable)")
	memorySourcesStageCmd.Flags().String("key", "", "Staging idempotency key (default: derived from the bytes)")
	memorySourcesCmd.AddCommand(memorySourcesStageCmd, memorySourcesGetCmd)

	memoryObserveCmd.Flags().String("source", "", "source_ref to observe")
	addMemoryIntentFlags(memoryObserveCmd, true)
	memoryReviseCmd.Flags().String("source", "", "source_ref describing the revision")
	memoryReviseCmd.Flags().String("kind", "", "correction, world_change, misrecorded or unspecified")
	memoryReviseCmd.Flags().String("target", "", "The claim being revised (required to repair a misrecording)")
	addMemoryIntentFlags(memoryReviseCmd, true)
	memoryFeedbackCmd.Flags().String("source", "", "source_ref of the feedback")
	memoryFeedbackCmd.Flags().String("decision", "", "Decision reference")
	memoryFeedbackCmd.Flags().String("attempt", "", "Attempt reference")
	addMemoryIntentFlags(memoryFeedbackCmd, true)
	memoryRecallCmd.Flags().String("mode", "", "answer (default), action, resume or attention")
	memoryRecallCmd.Flags().StringArray("after", nil, "Receipt to wait on (repeatable; the session's are added)")
	memoryRecallCmd.Flags().String("target", "", "Expand a basis_ref or item ref")
	memoryRecallCmd.Flags().String("detail", "", "brief or evidence")
	memoryRecallCmd.Flags().String("situation", "", "Transient context for this recall (never stored)")
	memoryRecallCmd.Flags().String("attention-cursor", "", "Attention cursor (default: the session's)")
	memoryRecallCmd.Flags().String("valid-at", "", "World time to answer for (KIP timestamp)")
	memoryRecallCmd.Flags().Uint64("as-of-seq", 0, "Cognitive history to answer at")
	addMemoryIntentFlags(memoryRecallCmd, false)

	// `memory forget --mode payload_only|semantic <target>` is the Memory
	// Interface forget; without --mode it stays the technical element purge.
	memoryForgetCmd.Flags().String("mode", "", "Memory Interface forget: payload_only or semantic (a single target)")
	addMemoryIntentFlags(memoryForgetCmd, true)
	memoryCmd.AddCommand(memorySourcesCmd, memoryObserveCmd, memoryReviseCmd, memoryFeedbackCmd,
		memoryRecallCmd, memoryReceiptCmd, memoryPlanCmd)
}

// memoryInterfaceForget runs the Memory Interface forget for one target.
func memoryInterfaceForget(cmd *cobra.Command, target, mode string) error {
	if mode != "payload_only" && mode != "semantic" {
		return fmt.Errorf("--mode is payload_only or semantic")
	}
	return sendMemory(cmd, "forget", map[string]any{"target_ref": target, "mode": mode}, true)
}
