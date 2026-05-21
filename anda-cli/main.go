package main

import (
	_ "github.com/joho/godotenv/autoload"
	"github.com/ldclabs/anda-brain/anda-cli/cmd"
)

func main() {
	cmd.Execute()
}
