// ignore_for_file: deprecated_member_use
import 'package:flutter/material.dart';

class ShowsGrid extends StatelessWidget {
  const ShowsGrid({
    super.key,
    required this.shows,
    required this.cover,
    required this.onOpen,
  });
  final List<dynamic> shows;
  final Widget Function(dynamic) cover;
  final void Function(dynamic) onOpen;
  @override
  Widget build(BuildContext context) => GridView.count(
    crossAxisCount: 2,
    shrinkWrap: true,
    physics: const NeverScrollableScrollPhysics(),
    children: [
      for (final show in shows)
        Card(
          child: InkWell(
            onTap: () => onOpen(show),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [cover(show), Text(show['title'].toString())],
            ),
          ),
        ),
    ],
  );
}
