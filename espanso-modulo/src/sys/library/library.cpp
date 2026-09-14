/*
 * This file is part of espanso.
 *
 * Copyright (C) 2019-2021 Federico Terzi
 *
 * espanso is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * espanso is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with espanso.  If not, see <https://www.gnu.org/licenses/>.
 */

#define _UNICODE

#include "../common/common.h"
#include "../interop/interop.h"

#include <vector>

const int LIBRARY_MIN_WIDTH = 720;
const int LIBRARY_MIN_HEIGHT = 460;
const int LIBRARY_RESULT_BUFFER_SIZE = 4096;
const size_t LIBRARY_PREVIEW_LENGTH = 48;

LibraryMetadata *libraryMetadata = nullptr;
LibraryActionCallback libraryActionCallback = nullptr;
void *libraryActionData = nullptr;

struct LibraryItem {
    wxString id;
    wxString collection;
    wxString trigger;
    wxString replacement;
    wxString label;
    bool editable = false;
};

static wxString FromMetadata(const char *value) {
    return value ? wxString::FromUTF8(value) : wxString();
}

class LibraryApp : public wxApp {
  public:
    virtual bool OnInit();
};

class LibraryFrame : public wxFrame {
  public:
    LibraryFrame();

  private:
    std::vector<LibraryItem> items;
    // Indexes into `items`, in the order they are displayed in the list.
    std::vector<int> visibleItems;
    // Index into `items` of the snippet shown in the editor, -1 for a new one.
    int selectedItem = -1;
    bool dirty = false;

    wxPanel *panel = nullptr;
    wxTextCtrl *searchCtrl = nullptr;
    wxListBox *listBox = nullptr;
    wxButton *newButton = nullptr;
    wxTextCtrl *triggerCtrl = nullptr;
    wxTextCtrl *labelCtrl = nullptr;
    wxTextCtrl *replacementCtrl = nullptr;
    wxStaticText *infoText = nullptr;
    wxButton *saveButton = nullptr;
    wxButton *deleteButton = nullptr;
    wxButton *openConfigButton = nullptr;
    wxCheckBox *startupCheck = nullptr;

    void LoadItems();
    void RefreshList(int preferredItem);
    void ShowItem(int index);
    void ShowNewItem();
    void UpdateInfo();
    bool ConfirmDiscard();
    bool SaveCurrent();
    bool InvokeAction(int action, const wxString *id, const wxString &trigger,
                      const wxString &replacement, const wxString &label,
                      bool startupEnabled, wxString &result);

    static wxString ItemLabel(const LibraryItem &item);
    static bool MatchesQuery(const LibraryItem &item, const wxString &query);

    void OnSearch(wxCommandEvent &event);
    void OnSelect(wxCommandEvent &event);
    void OnNew(wxCommandEvent &event);
    void OnSave(wxCommandEvent &event);
    void OnDelete(wxCommandEvent &event);
    void OnStartupToggle(wxCommandEvent &event);
    void OnOpenConfig(wxCommandEvent &event);
    void OnEdit(wxCommandEvent &event);
    void OnCharHook(wxKeyEvent &event);
    void OnClose(wxCloseEvent &event);
};

bool LibraryApp::OnInit() {
    LibraryFrame *frame = new LibraryFrame();

    if (libraryMetadata->window_icon_path) {
        setFrameIcon(wxString::FromUTF8(libraryMetadata->window_icon_path),
                     frame);
    }

    frame->Show(true);
    Activate(frame);

    return true;
}

LibraryFrame::LibraryFrame()
    : wxFrame(NULL, wxID_ANY, "Espanso Snippet Library", wxDefaultPosition,
              wxSize(960, 600), wxDEFAULT_FRAME_STYLE) {
    panel = new wxPanel(this, wxID_ANY);
    wxBoxSizer *rootSizer = new wxBoxSizer(wxVERTICAL);
    wxBoxSizer *bodySizer = new wxBoxSizer(wxHORIZONTAL);

    // Left column: search + list
    wxBoxSizer *leftSizer = new wxBoxSizer(wxVERTICAL);
    searchCtrl = new wxTextCtrl(panel, wxID_ANY);
    searchCtrl->SetHint("Search snippets");
    leftSizer->Add(searchCtrl, 0, wxEXPAND | wxBOTTOM, 6);

    listBox = new wxListBox(panel, wxID_ANY, wxDefaultPosition, wxSize(320, -1),
                            0, nullptr, wxLB_SINGLE);
    leftSizer->Add(listBox, 1, wxEXPAND | wxBOTTOM, 6);

    newButton = new wxButton(panel, wxID_NEW, "New snippet");
    leftSizer->Add(newButton, 0, wxEXPAND);
    bodySizer->Add(leftSizer, 2, wxEXPAND | wxALL, 10);

    // Right column: editor
    wxBoxSizer *rightSizer = new wxBoxSizer(wxVERTICAL);
    rightSizer->Add(new wxStaticText(panel, wxID_ANY, "Trigger"), 0, wxBOTTOM,
                    2);
    triggerCtrl = new wxTextCtrl(panel, wxID_ANY);
    rightSizer->Add(triggerCtrl, 0, wxEXPAND | wxBOTTOM, 8);

    rightSizer->Add(new wxStaticText(panel, wxID_ANY, "Label (optional)"), 0,
                    wxBOTTOM, 2);
    labelCtrl = new wxTextCtrl(panel, wxID_ANY);
    rightSizer->Add(labelCtrl, 0, wxEXPAND | wxBOTTOM, 8);

    rightSizer->Add(new wxStaticText(panel, wxID_ANY, "Replacement"), 0,
                    wxBOTTOM, 2);
    replacementCtrl =
        new wxTextCtrl(panel, wxID_ANY, wxEmptyString, wxDefaultPosition,
                       wxDefaultSize, wxTE_MULTILINE);
    replacementCtrl->SetFont(
        wxFont(wxFontInfo().Family(wxFONTFAMILY_TELETYPE)));
    rightSizer->Add(replacementCtrl, 1, wxEXPAND | wxBOTTOM, 8);

    infoText = new wxStaticText(panel, wxID_ANY, wxEmptyString);
    rightSizer->Add(infoText, 0, wxEXPAND | wxBOTTOM, 8);

    wxBoxSizer *actionSizer = new wxBoxSizer(wxHORIZONTAL);
    deleteButton = new wxButton(panel, wxID_DELETE, "Delete");
    actionSizer->Add(deleteButton, 0, wxRIGHT, 6);
    actionSizer->AddStretchSpacer(1);
    saveButton = new wxButton(panel, wxID_SAVE, "Save");
    actionSizer->Add(saveButton, 0);
    rightSizer->Add(actionSizer, 0, wxEXPAND);

    bodySizer->Add(rightSizer, 3, wxEXPAND | wxTOP | wxRIGHT | wxBOTTOM, 10);
    rootSizer->Add(bodySizer, 1, wxEXPAND);

    // Footer: startup control + config folder
    wxBoxSizer *footerSizer = new wxBoxSizer(wxHORIZONTAL);
    if (libraryMetadata->startup_supported) {
        startupCheck =
            new wxCheckBox(panel, wxID_ANY, "Start Espanso at login");
        startupCheck->SetValue(libraryMetadata->startup_enabled != 0);
        footerSizer->Add(startupCheck, 0, wxALIGN_CENTER_VERTICAL | wxRIGHT,
                         12);
    }
    footerSizer->AddStretchSpacer(1);
    openConfigButton = new wxButton(panel, wxID_ANY, "Open config folder");
    footerSizer->Add(openConfigButton, 0, wxALIGN_CENTER_VERTICAL);
    rootSizer->Add(footerSizer, 0, wxEXPAND | wxLEFT | wxRIGHT | wxBOTTOM, 10);

    panel->SetSizer(rootSizer);

    searchCtrl->Bind(wxEVT_TEXT, &LibraryFrame::OnSearch, this);
    listBox->Bind(wxEVT_LISTBOX, &LibraryFrame::OnSelect, this);
    newButton->Bind(wxEVT_BUTTON, &LibraryFrame::OnNew, this);
    saveButton->Bind(wxEVT_BUTTON, &LibraryFrame::OnSave, this);
    deleteButton->Bind(wxEVT_BUTTON, &LibraryFrame::OnDelete, this);
    openConfigButton->Bind(wxEVT_BUTTON, &LibraryFrame::OnOpenConfig, this);
    if (startupCheck) {
        startupCheck->Bind(wxEVT_CHECKBOX, &LibraryFrame::OnStartupToggle,
                           this);
    }
    triggerCtrl->Bind(wxEVT_TEXT, &LibraryFrame::OnEdit, this);
    labelCtrl->Bind(wxEVT_TEXT, &LibraryFrame::OnEdit, this);
    replacementCtrl->Bind(wxEVT_TEXT, &LibraryFrame::OnEdit, this);
    Bind(wxEVT_CHAR_HOOK, &LibraryFrame::OnCharHook, this);
    Bind(wxEVT_CLOSE_WINDOW, &LibraryFrame::OnClose, this);

    LoadItems();
    if (items.empty()) {
        RefreshList(-1);
        ShowNewItem();
    } else {
        ShowItem(0);
        RefreshList(0);
    }

    SetMinSize(wxSize(LIBRARY_MIN_WIDTH, LIBRARY_MIN_HEIGHT));
    CentreOnScreen();
}

void LibraryFrame::LoadItems() {
    items.clear();
    if (!libraryMetadata->entries) {
        return;
    }

    for (int i = 0; i < libraryMetadata->entries_count; i++) {
        const LibraryEntryMetadata &entry = libraryMetadata->entries[i];
        LibraryItem item;
        item.id = FromMetadata(entry.id);
        item.collection = FromMetadata(entry.collection);
        item.trigger = FromMetadata(entry.trigger);
        item.replacement = FromMetadata(entry.replacement);
        item.label = FromMetadata(entry.label);
        item.editable = entry.editable != 0;
        items.push_back(item);
    }
}

wxString LibraryFrame::ItemLabel(const LibraryItem &item) {
    wxString description = item.label;
    if (description.IsEmpty()) {
        description = item.replacement.BeforeFirst('\n');
        description.Replace("\r", "");
    }
    if (description.Length() > LIBRARY_PREVIEW_LENGTH) {
        description = description.Left(LIBRARY_PREVIEW_LENGTH - 3) + "...";
    }

    wxString text = item.trigger;
    if (!description.IsEmpty()) {
        text += "  -  " + description;
    }
    if (!item.editable) {
        text += "  (read-only)";
    }
    return text;
}

bool LibraryFrame::MatchesQuery(const LibraryItem &item,
                                const wxString &query) {
    if (query.IsEmpty()) {
        return true;
    }
    wxString needle = query.Lower();
    return item.trigger.Lower().Contains(needle) ||
           item.label.Lower().Contains(needle) ||
           item.replacement.Lower().Contains(needle) ||
           item.collection.Lower().Contains(needle);
}

void LibraryFrame::RefreshList(int preferredItem) {
    wxString query = searchCtrl->GetValue();
    query.Trim(true).Trim(false);

    visibleItems.clear();
    wxArrayString labels;
    int selection = wxNOT_FOUND;
    for (size_t i = 0; i < items.size(); i++) {
        if (!MatchesQuery(items[i], query)) {
            continue;
        }
        if ((int)i == preferredItem) {
            selection = (int)visibleItems.size();
        }
        visibleItems.push_back((int)i);
        labels.Add(ItemLabel(items[i]));
    }

    listBox->Freeze();
    listBox->Clear();
    if (!labels.IsEmpty()) {
        listBox->Append(labels);
    }
    listBox->SetSelection(selection);
    listBox->Thaw();
}

void LibraryFrame::ShowItem(int index) {
    selectedItem = index;
    const LibraryItem &item = items[index];

    // ChangeValue does not emit wxEVT_TEXT, so the editor stays clean.
    triggerCtrl->ChangeValue(item.trigger);
    labelCtrl->ChangeValue(item.label);
    replacementCtrl->ChangeValue(item.replacement);
    dirty = false;

    triggerCtrl->SetEditable(item.editable);
    labelCtrl->SetEditable(item.editable);
    replacementCtrl->SetEditable(item.editable);
    saveButton->Enable(item.editable);
    deleteButton->Enable(item.editable);
    UpdateInfo();
}

void LibraryFrame::ShowNewItem() {
    selectedItem = -1;

    triggerCtrl->ChangeValue(wxEmptyString);
    labelCtrl->ChangeValue(wxEmptyString);
    replacementCtrl->ChangeValue(wxEmptyString);
    dirty = false;

    triggerCtrl->SetEditable(true);
    labelCtrl->SetEditable(true);
    replacementCtrl->SetEditable(true);
    saveButton->Enable(true);
    deleteButton->Enable(false);
    listBox->SetSelection(wxNOT_FOUND);
    UpdateInfo();
    triggerCtrl->SetFocus();
}

void LibraryFrame::UpdateInfo() {
    wxString text;
    if (selectedItem < 0) {
        text = "New snippet. It will be saved to " +
               FromMetadata(libraryMetadata->new_entry_collection);
    } else {
        const LibraryItem &item = items[selectedItem];
        text = "Stored in " + item.collection;
        if (!item.editable) {
            text += ". Read-only: only files managed by the library can be "
                    "edited here.";
        }
    }
    infoText->SetLabel(text);
    panel->Layout();
}

bool LibraryFrame::ConfirmDiscard() {
    if (!dirty) {
        return true;
    }

    wxMessageDialog dialog(this, "This snippet has unsaved changes.",
                           "Unsaved changes",
                           wxYES_NO | wxCANCEL | wxICON_QUESTION);
    dialog.SetYesNoCancelLabels("Save", "Discard", "Cancel");
    int answer = dialog.ShowModal();
    if (answer == wxID_YES) {
        return SaveCurrent();
    }
    if (answer == wxID_NO) {
        dirty = false;
        return true;
    }
    return false;
}

bool LibraryFrame::InvokeAction(int action, const wxString *id,
                                const wxString &trigger,
                                const wxString &replacement,
                                const wxString &label, bool startupEnabled,
                                wxString &result) {
    result.Clear();
    if (!libraryActionCallback) {
        result = "The library backend is not available.";
        return false;
    }

    char buffer[LIBRARY_RESULT_BUFFER_SIZE];
    buffer[0] = '\0';

    wxCharBuffer idBuffer;
    if (id) {
        idBuffer = id->ToUTF8();
    }
    const wxCharBuffer triggerBuffer = trigger.ToUTF8();
    const wxCharBuffer replacementBuffer = replacement.ToUTF8();
    const wxCharBuffer labelBuffer = label.ToUTF8();

    int code = libraryActionCallback(
        action, id ? idBuffer.data() : nullptr, triggerBuffer.data(),
        replacementBuffer.data(), labelBuffer.data(), startupEnabled ? 1 : 0,
        libraryActionData, buffer, LIBRARY_RESULT_BUFFER_SIZE);

    buffer[LIBRARY_RESULT_BUFFER_SIZE - 1] = '\0';
    result = wxString::FromUTF8(buffer);
    return code == 0;
}

bool LibraryFrame::SaveCurrent() {
    wxString trigger = triggerCtrl->GetValue();
    wxString replacement = replacementCtrl->GetValue();
    wxString label = labelCtrl->GetValue();
    label.Trim(true).Trim(false);

    if (trigger.Strip(wxString::both).IsEmpty()) {
        wxMessageBox("Please enter a trigger.", "Missing trigger",
                     wxOK | wxICON_WARNING, this);
        triggerCtrl->SetFocus();
        return false;
    }
    if (replacement.IsEmpty()) {
        wxMessageBox("Please enter the replacement text.",
                     "Missing replacement", wxOK | wxICON_WARNING, this);
        replacementCtrl->SetFocus();
        return false;
    }

    wxString result;
    const wxString *id = selectedItem >= 0 ? &items[selectedItem].id : nullptr;
    if (!InvokeAction(LIBRARY_ACTION_SAVE, id, trigger, replacement, label,
                      false, result)) {
        wxMessageBox(result.IsEmpty() ? wxString("Unable to save the snippet.")
                                      : result,
                     "Save failed", wxOK | wxICON_ERROR, this);
        return false;
    }

    if (selectedItem >= 0) {
        LibraryItem &item = items[selectedItem];
        item.trigger = trigger;
        item.replacement = replacement;
        item.label = label;
        if (!result.IsEmpty()) {
            item.id = result;
        }
    } else {
        LibraryItem item;
        item.id = result;
        item.collection = FromMetadata(libraryMetadata->new_entry_collection);
        item.trigger = trigger;
        item.replacement = replacement;
        item.label = label;
        item.editable = true;
        items.push_back(item);
        selectedItem = (int)items.size() - 1;
    }

    dirty = false;
    ShowItem(selectedItem);
    RefreshList(selectedItem);
    return true;
}

void LibraryFrame::OnSearch(wxCommandEvent &event) {
    RefreshList(selectedItem);
}

void LibraryFrame::OnSelect(wxCommandEvent &event) {
    int selection = event.GetSelection();
    if (selection == wxNOT_FOUND || selection >= (int)visibleItems.size()) {
        return;
    }

    int target = visibleItems[selection];
    if (target == selectedItem) {
        return;
    }

    if (!ConfirmDiscard()) {
        RefreshList(selectedItem);
        return;
    }

    ShowItem(target);
    RefreshList(target);
}

void LibraryFrame::OnNew(wxCommandEvent &event) {
    if (!ConfirmDiscard()) {
        return;
    }
    ShowNewItem();
}

void LibraryFrame::OnSave(wxCommandEvent &event) { SaveCurrent(); }

void LibraryFrame::OnDelete(wxCommandEvent &event) {
    if (selectedItem < 0) {
        return;
    }

    wxMessageDialog dialog(
        this,
        wxString::Format("Delete the snippet \"%s\"?",
                         items[selectedItem].trigger),
        "Delete snippet", wxYES_NO | wxNO_DEFAULT | wxICON_WARNING);
    if (dialog.ShowModal() != wxID_YES) {
        return;
    }

    wxString result;
    if (!InvokeAction(LIBRARY_ACTION_DELETE, &items[selectedItem].id,
                      wxEmptyString, wxEmptyString, wxEmptyString, false,
                      result)) {
        wxMessageBox(result.IsEmpty()
                         ? wxString("Unable to delete the snippet.")
                         : result,
                     "Delete failed", wxOK | wxICON_ERROR, this);
        return;
    }

    items.erase(items.begin() + selectedItem);
    dirty = false;
    RefreshList(-1);
    ShowNewItem();
}

void LibraryFrame::OnStartupToggle(wxCommandEvent &event) {
    bool enabled = startupCheck->GetValue();
    wxString result;
    if (!InvokeAction(LIBRARY_ACTION_SET_STARTUP, nullptr, wxEmptyString,
                      wxEmptyString, wxEmptyString, enabled, result)) {
        startupCheck->SetValue(!enabled);
        wxMessageBox(result.IsEmpty()
                         ? wxString("Unable to update the startup setting.")
                         : result,
                     "Startup setting", wxOK | wxICON_ERROR, this);
    }
}

void LibraryFrame::OnOpenConfig(wxCommandEvent &event) {
    if (libraryMetadata->config_dir) {
        wxLaunchDefaultApplication(
            wxString::FromUTF8(libraryMetadata->config_dir));
    }
}

void LibraryFrame::OnEdit(wxCommandEvent &event) {
    dirty = true;
    event.Skip();
}

void LibraryFrame::OnCharHook(wxKeyEvent &event) {
    if (event.GetKeyCode() == WXK_ESCAPE) {
        Close();
        return;
    }

    // ControlDown maps to Cmd on macOS and Ctrl elsewhere.
    if (event.ControlDown() && event.GetKeyCode() == 'S') {
        if (saveButton->IsEnabled()) {
            SaveCurrent();
        }
        return;
    }

    if (event.ControlDown() && event.GetKeyCode() == 'N') {
        if (ConfirmDiscard()) {
            ShowNewItem();
        }
        return;
    }

    event.Skip();
}

void LibraryFrame::OnClose(wxCloseEvent &event) {
    if (event.CanVeto() && !ConfirmDiscard()) {
        event.Veto();
        return;
    }
    event.Skip();
}

extern "C" void interop_show_library(LibraryMetadata *_metadata,
                                     LibraryActionCallback _callback,
                                     void *_data) {
#ifdef __WXMSW__
    SetProcessDPIAware();
#endif

    libraryMetadata = _metadata;
    libraryActionCallback = _callback;
    libraryActionData = _data;

    wxApp::SetInstance(new LibraryApp());
    int argc = 0;
    wxEntry(argc, (char **)nullptr);
}
