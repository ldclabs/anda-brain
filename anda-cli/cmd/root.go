package cmd

import (
	"encoding/json"
	"fmt"
	"os"
	"strconv"
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

const Version = "0.9.0"

func newClient() *api.Client {
	client := api.NewClient(baseURL, spaceID, token)
	client.Shard = shard
	if timeoutSec > 0 {
		client.HTTPClient.Timeout = time.Duration(timeoutSec) * time.Second
	}
	return client
}

func printJSON(v any) {
	data, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}
	fmt.Println(string(data))
}

func exitError(err error) {
	fmt.Fprintf(os.Stderr, "Error: %v\n", err)
	os.Exit(1)
}

var rootCmd = &cobra.Command{
	Use:     "anda-cli",
	Short:   "CLI tool for Anda Brain API",
	Long:    "A command-line interface for interacting with the Anda Brain memory service.",
	Version: Version,
}

func Execute() {
	if err := rootCmd.Execute(); err != nil {
		os.Exit(1)
	}
}

func init() {
	rootCmd.PersistentFlags().StringVar(&baseURL, "base-url", envOrDefault("ANDA_BASE_URL", api.DefaultBaseURL), "API base URL (env: ANDA_BASE_URL)")
	rootCmd.PersistentFlags().StringVar(&spaceID, "space-id", os.Getenv("ANDA_SPACE_ID"), "Space ID (env: ANDA_SPACE_ID)")
	rootCmd.PersistentFlags().StringVar(&token, "token", os.Getenv("ANDA_TOKEN"), "Auth token (env: ANDA_TOKEN)")
	rootCmd.PersistentFlags().IntVar(&shard, "shard", envOrDefaultInt("ANDA_SHARD", 0), "Shard index sent as Shard-Id header for sharded deployments (env: ANDA_SHARD)")
	rootCmd.PersistentFlags().IntVar(&timeoutSec, "timeout", envOrDefaultInt("ANDA_TIMEOUT", 120), "HTTP request timeout in seconds (env: ANDA_TIMEOUT)")
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
