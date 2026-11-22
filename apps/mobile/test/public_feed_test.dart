import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/features/listen/public_feed.dart';

void main() {
  test('reads HTTPS enclosure and cover from a known public feed', () {
    final show = <String, dynamic>{'id': 1, 'title': 'Show'};
    final items = parsePublicFeed(
      '<rss><channel><image><url>https://example.com/cover.jpg</url></image><item><title>Episode</title><enclosure url="https://example.com/audio.mp3"/></item><item><title>Insecure</title><enclosure url="http://example.com/audio.mp3"/></item></channel></rss>',
      show,
    );
    expect(show['public_cover'], 'https://example.com/cover.jpg');
    expect(items.single['title'], 'Episode');
    expect(items.single['enclosure'], 'https://example.com/audio.mp3');
  });
}
