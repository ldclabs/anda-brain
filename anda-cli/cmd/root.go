package cmd

import (
	"encoding/json"
	"fmt"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/ldclabs/anda-brain/anda-cli/api"
	"github.com/spf13/cobra"
)

var (
	baseURL    string
	spaceID    string
	token      string
	shard      int
	timeoutSec int
)

const Version = "0.13.7"

func newClient() *api.Client {
	client := api.NewClient(baseURL, spaceID, token)
	client.Shard = shard
	if timeoutSec > 0 {
		client.HTTPClient.Timeout = time.Duration(timeoutSec) * time.Second
	}
	return client
}

func printJSON(cmd *cobra.Command, v any) error {
	encoder := json.NewEncoder(cmd.OutOrStdout())
	encoder.SetIndent("", "  ")
	return encoder.Encode(v)
}

var rootCmd = &cobra.Command{
	Use:           "anda-cli",
	Short:         "CLI tool for Anda Brain API",
	Long:          "A command-line interface for interacting with the Anda Brain memory service.",
	Version:       Version,
	SilenceUsage:  true,
	SilenceErrors: true,
	PersistentPreRunE: func(cmd *cobra.Command, args []string) error {
		if timeoutSec <= 0 {
			return fmt.Errorf("--timeout must be positive")
		}
		if shard < 0 {
			return fmt.Errorf("--shard must be non-negative")
		}
		token = secretFlag(cmd, "token", "ANDA_TOKEN")
		if requiresSpace(cmd) && strings.TrimSpace(spaceID) == "" {
			return fmt.Errorf("--space-id (or ANDA_SPACE_ID) is required")
		}
		return nil
	},
}

// spaceFree marks a command, and every command under it, as not addressing a
// Space, so the root does not require --space-id for it.
var spaceFree = map[string]string{"space": "none"}

func requiresSpace(cmd *cobra.Command) bool {
	for c := cmd; c != nil; c = c.Parent() {
		if c.Annotations["space"] == "none" {
			return false
		}
		switch c.Name() {
		// Cobra's built-in help and shell completion commands.
		case "help", "completion", cobra.ShellCompRequestCmd, cobra.ShellCompNoDescRequestCmd:
			return false
		}
	}
	return true
}

func Execute() error {
	return rootCmd.Execute()
}

func init() {
	rootCmd.PersistentFlags().StringVar(&baseURL, "base-url", envOrDefault("ANDA_BASE_URL", api.DefaultBaseURL), "API base URL (env: ANDA_BASE_URL)")
	rootCmd.PersistentFlags().StringVar(&spaceID, "space-id", os.Getenv("ANDA_SPACE_ID"), "Space ID (env: ANDA_SPACE_ID)")
	rootCmd.PersistentFlags().StringVar(&token, "token", "", "Auth token (env: ANDA_TOKEN)")
	rootCmd.PersistentFlags().IntVar(&shard, "shard", envOrDefaultInt("ANDA_SHARD", 0), "Shard index sent as Shard-Id header for sharded deployments (env: ANDA_SHARD)")
	rootCmd.PersistentFlags().IntVar(&timeoutSec, "timeout", envOrDefaultInt("ANDA_TIMEOUT", 120), "HTTP request timeout in seconds (env: ANDA_TIMEOUT)")
}

// Secret environment values must never become flag defaults: Cobra includes
// defaults in help and usage text. An explicitly supplied flag always wins.
func secretFlag(cmd *cobra.Command, name, env string) string {
	if cmd.Flags().Changed(name) {
		value, _ := cmd.Flags().GetString(name)
		return value
	}
	return os.Getenv(env)
}

func envOrDefault(key, defaultVal string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return defaultVal
}

func envOrDefaultInt(key string, defaultVal int) int {
	if v := os.Getenv(key); v != "" {
		if n, err := strconv.Atoi(v); err == nil {
			return n
		}
		fmt.Fprintf(os.Stderr, "Warning: invalid %s=%q, using default %d\n", key, v, defaultVal)
	}
	return defaultVal
}
