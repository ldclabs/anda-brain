package cmd

import (
	"fmt"
	"strings"

	"github.com/ldclabs/anda-brain/anda-cli/api"
	"github.com/spf13/cobra"
)

var executeKIPReadonlyCmd = &cobra.Command{
	Use:   "execute-kip-readonly",
	Short: "Execute a read-only KIP request",
	Long: `Execute a KIP request in read-only mode.

Input can be provided via --request, --file, or stdin: a JSON request (or
--request @file), or one bare KIP command. The JSON request accepts either a
single "command" string or an "operations" array. More than one operation
requires "execution":{"mode":"independent"}.

Example:
  anda-cli --space-id my_space --token $TOKEN execute-kip-readonly \
		--request 'DESCRIBE PRIMER'

  anda-cli --space-id my_space --token $TOKEN execute-kip-readonly \
		--request '{"operations":["DESCRIBE PRIMER","DESCRIBE SCHEMA ENVIRONMENT"],"execution":{"mode":"independent"}}'

  anda-cli --space-id my_space --token $TOKEN execute-kip-readonly --file ./kip_request.json

  cat kip_request.json | anda-cli --space-id my_space --token $TOKEN execute-kip-readonly`,
	Args: cobra.NoArgs,
	RunE: func(cmd *cobra.Command, args []string) error {
		raw, err := readInput(cmd, "request", "file")
		if err != nil {
			return err
		}
		raw = strings.TrimSpace(raw)
		var input api.KipRequest
		switch {
		case raw == "":
			return fmt.Errorf("empty KIP request")
		case strings.HasPrefix(raw, "{") || strings.HasPrefix(raw, "@"):
			input, err = readJSONObject[api.KipRequest](raw)
			if err != nil {
				return fmt.Errorf("invalid request JSON: %w", err)
			}
		default:
			// The server reads a bare command as {"command": ...} too.
			input.Command = raw
		}
		input.Command = strings.TrimSpace(input.Command)
		if input.Command == "" && len(input.Operations) == 0 {
			return fmt.Errorf("invalid request JSON: either command or operations is required")
		}
		if input.Command != "" && len(input.Operations) > 0 {
			return fmt.Errorf("invalid request JSON: command and operations are mutually exclusive")
		}
		if len(input.Operations) > 1 && input.Execution == nil {
			return fmt.Errorf("invalid request JSON: multiple operations require execution.mode")
		}
		if input.Execution != nil {
			switch input.Execution.Mode {
			case "independent", "sequence", "atomic":
			default:
				return fmt.Errorf("invalid execution.mode %q", input.Execution.Mode)
			}
		}

		resp, err := newClient().ExecuteKIPReadonly(cmd.Context(), &input)
		if resp != nil {
			if printErr := printJSON(cmd, resp); printErr != nil {
				return printErr
			}
		}
		if err != nil {
			return err
		}
		return resp.Failure()
	},
}

func init() {
	executeKIPReadonlyCmd.Flags().String("request", "", "KIP request JSON, @file, or one KIP command")
	executeKIPReadonlyCmd.Flags().String("file", "", "Read the KIP request from a file")
	rootCmd.AddCommand(executeKIPReadonlyCmd)
}
