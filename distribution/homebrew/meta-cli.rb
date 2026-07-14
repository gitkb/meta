# typed: false
# frozen_string_literal: true

class MetaCli < Formula
  desc "Multi-repo management CLI with AI integration"
  homepage "https://github.com/gitkb/meta"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/gitkb/meta/releases/download/v#{version}/meta-darwin-arm64.tar.gz"
      # sha256 will be filled in by release automation
    end
    on_intel do
      url "https://github.com/gitkb/meta/releases/download/v#{version}/meta-darwin-x64.tar.gz"
      # sha256 will be filled in by release automation
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/gitkb/meta/releases/download/v#{version}/meta-linux-arm64.tar.gz"
      # sha256 will be filled in by release automation
    end
    on_intel do
      url "https://github.com/gitkb/meta/releases/download/v#{version}/meta-linux-x64.tar.gz"
      # sha256 will be filled in by release automation
    end
  end

  def install
    bin.install "meta"
    bin.install "meta-git"
    bin.install "meta-project"
    bin.install "meta-rust"
    bin.install "meta-mcp"
    bin.install "loop"
  end

  test do
    system "#{bin}/meta", "--version"
  end
end
