class McrawTui < Formula
  desc "Cross Platform TUI for encoding your motioncam MCRAW files to professional video formats. All in the Terminal."
  homepage "https://github.com/Yoganshbhatt/mcraw-tui"
  version "0.2.4"
  license "Apache-2.0"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.4/mcraw-tui-aarch64-apple-darwin.zip"
    sha256 "865669B08A99E237D6225F7F4CC82FFAC0B1B56D7691B6F496B44F0660F18962"
  elsif OS.mac? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.4/mcraw-tui-x86_64-apple-darwin.zip"
    sha256 "6B61A70F3D8AA5FAEBEC688FF5640E32C11CEF4B8C1CCB83E3B0916A3E60EA60"
  elsif OS.linux? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.4/mcraw-tui-x86_64-unknown-linux-gnu.zip"
    sha256 "4EC865E15FA0939122DA1EF90CF213F62098AD5B37E4D0FEEED43F103724CD97"
  end

  depends_on "ffmpeg"

  def install
    bin.install "mcraw-tui"
  end
end
