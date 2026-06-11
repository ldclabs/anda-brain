package cmd

import (
	"github.com/spf13/cobra"
)

var statusCmd = &cobra.Command{
	Use:   "status",
	Short: "Get service information (name, version, sharding)",
	Run: func(cmd *cobra.Command, args []string) {
		client := newClient()
		info, err := client.GetInfo(cmd.Context())
		if err != nil {
			exitError(err)
		}
		printJSON(info)
	},
}

func init() {
	rootCmd.AddCommand(statusCmd)
}
