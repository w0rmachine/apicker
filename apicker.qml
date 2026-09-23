import Quickshell
import Quickshell.Io
import Quickshell.Widgets
import QtQuick
import QtQuick.Controls

PopupWindow {
    id: pickerPopup
    
    // Position anchoring (to be adjusted to fit your bar panel setup)
    // anchor.window: mainBar
    // anchor.rect.x: mainBar.width / 2 - width / 2
    // anchor.rect.y: mainBar.height
    
    implicitWidth: 500
    implicitHeight: 550
    visible: false // Hidden by default, toggled via IPC
    
    // Keep keyboard focus grabbed when visible
    grabFocus: visible
    
    property string currentSource: ""

    // Re-run the query, cancelling any in-flight search.
    function scheduleSearch() {
        searchDebounce.restart();
    }

    // Debounce keystrokes so holding a key doesn't spawn one apicker per tick.
    Timer {
        id: searchDebounce
        interval: 60
        repeat: false
        onTriggered: searchProcess.restartSearch()
    }
    
    // IPC Interface to allow launching or toggling via shell keybindings
    // Run command: `qs ipc call apicker toggle`
    IpcHandler {
        target: "apicker"
        
        function toggle() {
            pickerPopup.visible = !pickerPopup.visible;
            if (pickerPopup.visible) {
                searchInput.text = "";
                pickerPopup.currentSource = "";
                searchInput.forceActiveFocus();
                pickerPopup.scheduleSearch();
            }
        }
        
        function show() {
            pickerPopup.visible = true;
            searchInput.text = "";
            pickerPopup.currentSource = "";
            searchInput.forceActiveFocus();
            pickerPopup.scheduleSearch();
        }
        
        function hide() {
            pickerPopup.visible = false;
        }
    }
    
    // Aesthetic Styling (Catppuccin Macchiato theme palette)
    Rectangle {
        anchors.fill: parent
        color: "#24273a"
        radius: 12
        border.color: "#363a4f"
        border.width: 1

        Column {
            anchors.fill: parent
            anchors.margins: 14
            spacing: 12

            // Header/Breadcrumb Bar
            Row {
                width: parent.width
                spacing: 6
                visible: pickerPopup.currentSource !== ""

                Rectangle {
                    color: "#f5bde6"
                    radius: 4
                    width: sourceText.contentWidth + 12
                    height: sourceText.contentHeight + 6
                    
                    Text {
                        id: sourceText
                        anchors.centerIn: parent
                        text: pickerPopup.currentSource.toUpperCase()
                        color: "#181926"
                        font.bold: true
                        font.pointSize: 9
                    }
                }
                
                Text {
                    text: ">"
                    color: "#a5adcb"
                    font.pointSize: 10
                    anchors.verticalCenter: parent.verticalCenter
                }
                
                Text {
                    text: "Search " + pickerPopup.currentSource
                    color: "#a5adcb"
                    font.pointSize: 10
                    anchors.verticalCenter: parent.verticalCenter
                }
            }

            // Search input field
            TextField {
                id: searchInput
                width: parent.width
                placeholderText: pickerPopup.currentSource !== "" ? "Search entries..." : "Search files or type ':' for sources..."
                placeholderTextColor: "#8087a2"
                focus: true
                
                background: Rectangle {
                    color: "#363a4f"
                    radius: 8
                    border.color: searchInput.activeFocus ? "#c6a0f6" : "#494d64"
                    border.width: 1
                }
                color: "#cad3f5"
                font.pointSize: 11
                leftPadding: 10
                rightPadding: 10
                
                onTextChanged: {
                    searchProcess.query = text;
                    pickerPopup.scheduleSearch();
                }
                
                // Keyboard navigation controls
                Keys.onPressed: (event) => {
                    if (event.key === Qt.Key_Down) {
                        resultsList.incrementCurrentIndex();
                        event.accepted = true;
                    } else if (event.key === Qt.Key_Up) {
                        resultsList.decrementCurrentIndex();
                        event.accepted = true;
                    } else if (event.key === Qt.Key_Escape) {
                        pickerPopup.visible = false;
                        event.accepted = true;
                    } else if (event.key === Qt.Key_Backspace && searchInput.text === "" && pickerPopup.currentSource !== "") {
                        // Exit current submenu back to root files menu when backspace is pressed on empty input
                        pickerPopup.currentSource = "";
                        pickerPopup.scheduleSearch();
                        event.accepted = true;
                    } else if (event.key === Qt.Key_Enter || event.key === Qt.Key_Return) {
                        if (resultsList.currentItem) {
                            resultsList.currentItem.trigger();
                        }
                        event.accepted = true;
                    }
                }
            }

            // Results Listing
            ListView {
                id: resultsList
                width: parent.width
                height: parent.height - searchInput.height - (pickerPopup.currentSource !== "" ? sourceText.height + 24 : 12) - 24
                clip: true
                model: ListModel { id: resultsModel }
                
                delegate: ItemDelegate {
                    width: parent.width
                    height: 38
                    highlighted: ListView.isCurrentItem
                    
                    function trigger() {
                        if (model.is_submenu) {
                            pickerPopup.currentSource = model.target_source;
                            searchInput.text = "";
                            pickerPopup.scheduleSearch();
                        } else {
                            actionProcess.runAction(model.path, pickerPopup.currentSource);
                            pickerPopup.visible = false;
                        }
                    }
                    
                    background: Rectangle {
                        color: parent.highlighted ? "#494d64" : parent.hovered ? "#363a4f" : "transparent"
                        radius: 6
                    }
                    
                    Row {
                        anchors.fill: parent
                        anchors.margins: 8
                        spacing: 10
                        
                        // Icon image
                        IconImage {
                            source: "image://icon/" + model.icon
                            width: 18
                            height: 18
                        }
                        
                        Text {
                            text: model.text
                            color: parent.parent.highlighted ? "#f4dbd6" : "#cad3f5"
                            elide: Text.ElideMiddle
                            font.pointSize: 10
                            anchors.verticalCenter: parent.verticalCenter
                        }
                    }
                    
                    onClicked: trigger()
                }
            }
        }
    }

    // Process for searching matching items
    Process {
        id: searchProcess
        property string query: ""
        
        command: {
            var args = ["apicker", "search", "--query", query];
            if (pickerPopup.currentSource !== "") {
                args.push("--source", pickerPopup.currentSource);
            }
            return args;
        }

        // Collect to completion; reading stdout mid-flight can hand JSON.parse
        // a half-written object.
        stdout: StdioCollector {
            id: searchStdout
            waitForEnd: true
        }
        
        onExited: (exitCode, exitStatus) => {
            if (exitCode !== 0) return;

            var output = searchStdout.text;
            if (!output) return;
            
            try {
                var items = JSON.parse(output);
                resultsModel.clear();
                for (var i = 0; i < items.length; i++) {
                    resultsModel.append(items[i]);
                }
                // `clear()` resets the index to -1; re-highlight the first hit.
                resultsList.currentIndex = items.length > 0 ? 0 : -1;
            } catch (e) {
                console.log("Failed to parse JSON: " + e);
            }
        }
        
        function restartSearch() {
            running = false;
            running = true;
        }
    }

    // Process for triggering selected actions
    Process {
        id: actionProcess
        
        function runAction(itemPath, sourceName) {
            var args = ["apicker", "execute", "--item", itemPath];
            if (sourceName !== "") {
                args.push("--source", sourceName);
            }
            command = args;
            running = true;
        }
    }
}
