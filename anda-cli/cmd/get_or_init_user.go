package cmd

import (
	"github.com/ldclabs/anda-brain/anda-cli/api"
	"github.com/spf13/cobra"
)

var getOrInitUserCmd = &cobra.Command{
	Use:   "get-or-init-user <user>",
	Short: "Get or initialize a user concept",
	Long: `Get or initialize a user concept node in the space.

This endpoint returns the raw concept object instead of an RPC envelope.

Example:
  anda-cli --space-id my_space --token $TOKEN get-or-init-user principal_123
  anda-cli --space-id my_space --token $TOKEN get-or-init-user principal_123 --name Alice`,
	Args: cobra.ExactArgs(1),
	Run: func(cmd *cobra.Command, args []string) {
		name, _ := cmd.Flags().GetString("name")

		input := &api.GetOrInitUserInput{
			User: args[0],
		}
		if name != "" {
			input.Name = &name
		}

		client := newClient()
		resp, err := client.GetOrInitUser(cmd.Context(), input)
		if err != nil {
			exitError(err)
		}
		if resp.Error != nil {
			exitError(resp.Error)
		}
		if resp.Result != nil {
			printJSON(resp.Result)
		}
	},
}

func init() {
	getOrInitUserCmd.Flags().String("name", "", "Optional display name used when creating the user concept")
	rootCmd.AddCommand(getOrInitUserCmd)
}
