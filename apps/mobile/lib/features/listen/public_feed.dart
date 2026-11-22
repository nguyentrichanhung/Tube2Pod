import 'package:xml/xml.dart';

List<Map<String, dynamic>> parsePublicFeed(String source, dynamic show) {
  final xml = XmlDocument.parse(source);
  final channel = xml.findAllElements('channel').firstOrNull;
  final title = channel?.findElements('title').firstOrNull?.innerText.trim();
  if (channel == null || title == null || title.isEmpty) {
    throw const FormatException('Feed RSS không hợp lệ');
  }
  if (show['title'] == null || show['title'] == '') {
    show['title'] = title;
    show['description'] =
        channel.findElements('description').firstOrNull?.innerText ?? '';
  }
  final image = channel
      .findElements('image')
      .firstOrNull
      ?.findElements('url')
      .firstOrNull
      ?.innerText;
  if (image != null && Uri.tryParse(image)?.scheme == 'https') {
    show['public_cover'] = image;
  }
  return xml
      .findAllElements('item')
      .map((item) {
        final enclosure = item
            .findElements('enclosure')
            .firstOrNull
            ?.getAttribute('url');
        return <String, dynamic>{
          'title': item.findElements('title').firstOrNull?.innerText ?? '',
          'show_title': show['title'],
          'show_id': show['id'],
          'enclosure': enclosure,
        };
      })
      .where(
        (e) =>
            e['enclosure'] != null &&
            Uri.tryParse(e['enclosure'] as String)?.scheme == 'https',
      )
      .toList();
}
