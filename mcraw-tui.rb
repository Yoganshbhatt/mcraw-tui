class McrawTui < Formula
  desc "Cross Platform TUI for encoding your motioncam MCRAW files to professional video formats. All in the Terminal."
  homepage "https://github.com/Yoganshbhatt/mcraw-tui"
  version "0.2.0"
  license "Apache-2.0"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.0/mcraw-tui-aarch64-apple-darwin.zip"
    sha256 "0266DC44E2538FE78C5A85CB8F882879157FE8081F410D0CABC5CD3E25BEFDDF"
  elsif OS.mac? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.0/mcraw-tui-x86_64-apple-darwin.zip"
    sha256 "F0E59480F25C8ADBD7D7136AE282363F688B222059B43DC5C941F89D5487E07E"
  elsif OS.linux? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.0/mcraw-tui-x86_64-unknown-linux-gnu.zip"
    sha256 "A90CF78C2F66B61442ED69E2AA70DC2BFAA5BB922224F2BB475083D39E9FBB72"
  end

  depends_on "ffmpeg"

  def install
    bin.install "mcraw-tui"
  end
end
