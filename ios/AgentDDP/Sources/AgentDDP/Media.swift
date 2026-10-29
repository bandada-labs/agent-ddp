#if DEBUG && canImport(UIKit)
import AVFoundation
import MediaPlayer
import UIKit

/// `media.list`: the players the platform shows the library, never a guess. Every `AVPlayerLayer` in the app's
/// windows with its player's state, and the now-playing info the app gave the system (lock screen, Control Center).
/// An audio-only player has no layer: the app shows it as a value (`AgentDDP.state`, see `ddp skills get app`).
enum Media {
    static func list() -> [String: Any] {
        onMain {
            var players: [[String: Any]] = []
            for window in FrameCapture.visibleWindows() {
                collect(window.layer, in: window, into: &players)
            }
            var answer: [String: Any] = ["players": players]
            if let info = MPNowPlayingInfoCenter.default().nowPlayingInfo {
                answer["nowPlaying"] = nowPlaying(info)
            }
            return answer
        }
    }

    private static func collect(_ layer: CALayer, in window: UIWindow, into players: inout [[String: Any]]) {
        if let playerLayer = layer as? AVPlayerLayer, let player = playerLayer.player {
            let frame = layer.convert(layer.bounds, to: window.layer)
            var entry = state(of: player)
            entry["frame"] = [frame.minX, frame.minY, frame.width, frame.height].map { ($0 * 100).rounded() / 100 }
            players.append(entry)
        }
        layer.sublayers?.forEach { collect($0, in: window, into: &players) }
    }

    private static func state(of player: AVPlayer) -> [String: Any] {
        let playing: String
        switch player.timeControlStatus {
        case .playing: playing = "playing"
        case .paused: playing = "paused"
        case .waitingToPlayAtSpecifiedRate: playing = "waiting"
        @unknown default: playing = "unknown"
        }
        var entry: [String: Any] = ["source": "AVPlayerLayer", "state": playing, "rate": player.rate,
                                    "position": seconds(player.currentTime()) ?? NSNull()]
        if let item = player.currentItem {
            entry["duration"] = seconds(item.duration) ?? NSNull()
            if let asset = item.asset as? AVURLAsset { entry["url"] = NetworkLog.shared.currentRedactor.url(asset.url) }
        }
        return entry
    }

    private static func nowPlaying(_ info: [String: Any]) -> [String: Any] {
        let keys: [(String, String)] = [(MPMediaItemPropertyTitle, "title"), (MPMediaItemPropertyArtist, "artist"),
                                        (MPMediaItemPropertyAlbumTitle, "album"), (MPMediaItemPropertyPlaybackDuration, "duration"),
                                        (MPNowPlayingInfoPropertyElapsedPlaybackTime, "position"),
                                        (MPNowPlayingInfoPropertyPlaybackRate, "rate")]
        return keys.reduce(into: [:]) { result, key in
            if let value = info[key.0], JSONSerialization.isValidJSONObject([value]) { result[key.1] = value }
        }
    }

    /// Seconds, rounded to milliseconds; nil for an indefinite time (a live stream, an item not loaded yet).
    private static func seconds(_ time: CMTime) -> Double? {
        guard time.isNumeric else { return nil }
        return (time.seconds * 1000).rounded() / 1000
    }
}
#endif
